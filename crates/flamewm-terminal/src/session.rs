//! One tab: a PTY process, its VT parser and its screen. A reader thread per PTY forwards output to the UI loop and
//! wakes it through a socket pair, the same pattern flamewm-explorer uses for its workers.
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

use crate::parser::Parser;
use crate::pty::{Launch, PtyProcess, PtySize};
use crate::screen::Screen;

/// What the reader threads report to the UI loop.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Output { id: u64, bytes: Vec<u8> },
    Exited { id: u64 },
}

pub struct Session {
    pub id: u64,
    pub pty: PtyProcess,
    parser: Parser,
    pub screen: Screen,
}

impl Session {
    /// Starts `launch` on a new PTY and a reader thread that reports to `events` and pokes `wake`.
    pub fn start(
        id: u64,
        launch: &Launch,
        size: PtySize,
        scrollback: usize,
        events: Sender<SessionEvent>,
        wake: UnixStream,
    ) -> Result<Self, String> {
        let mut pty = PtyProcess::spawn(launch, size).map_err(|error| format!("start {}: {error}", launch.program))?;
        let reader = pty.take_reader().ok_or("pty reader missing")?;
        let notice_wake = wake.try_clone().map_err(|error| error.to_string())?;
        std::thread::Builder::new()
            .name(format!("flamewm-terminal-read-{id}"))
            .spawn(move || read_loop(id, reader, events, wake, notice_wake))
            .map_err(|error| error.to_string())?;
        Ok(Self { id, pty, parser: Parser::new(), screen: Screen::new(size.cols as usize, size.rows as usize, scrollback) })
    }

    /// Feeds child output through the parser, then writes any replies (cursor reports, DA) back to the child.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(bytes, &mut self.screen);
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            let _ = self.pty.write(replies);
        }
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.pty.write(bytes);
    }

    /// Resizes the grid and tells the child the new size (TIOCSWINSZ sends SIGWINCH to the foreground group).
    pub fn resize(&mut self, cols: usize, rows: usize, pixel: (u16, u16)) {
        self.screen.resize(cols, rows);
        let _ = self.pty.resize(PtySize {
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: pixel.0,
            pixel_height: pixel.1,
        });
    }

    /// Working directory of the child, read from `/proc`; used to start new tabs in the same place.
    pub fn working_directory(&self) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.pty.id())).ok()
    }
}

/// Reports `Exited` when the reader thread ends for any reason, including a panic (Drop runs while unwinding),
/// so a dead PTY can never leave its tab open with nothing feeding it.
struct ExitNotice {
    id: u64,
    events: Sender<SessionEvent>,
    wake: UnixStream,
}

impl Drop for ExitNotice {
    fn drop(&mut self) {
        let _ = self.events.send(SessionEvent::Exited { id: self.id });
        let _ = self.wake.write_all(&[1]);
    }
}

fn read_loop(id: u64, mut reader: File, events: Sender<SessionEvent>, mut wake: UnixStream, notice_wake: UnixStream) {
    let _notice = ExitNotice { id, events: events.clone(), wake: notice_wake };
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                if events.send(SessionEvent::Output { id, bytes: buffer[..count].to_vec() }).is_err() {
                    return;
                }
                if wake.write_all(&[1]).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            // EIO is how Linux ends a PTY once the child's session is gone; any other error also ends the tab.
            Err(_) => return,
        }
    }
}
