//! One tab: a PTY process, its VT parser and its screen. A reader thread per PTY forwards output to the UI loop and
//! wakes it through a socket pair, the same pattern flamewm-explorer uses for its workers.
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

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
        let mut pty = PtyProcess::spawn(launch, size)
            .map_err(|error| format!("start {}: {error}", launch.program))?;
        let reader = pty.take_reader().ok_or("pty reader missing")?;
        let notice_wake = wake.try_clone().map_err(|error| error.to_string())?;
        let pacer = spawn_pacer(wake)?;
        std::thread::Builder::new()
            .name(format!("flamewm-terminal-read-{id}"))
            .spawn(move || read_loop(id, reader, events, pacer, notice_wake))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            id,
            pty,
            parser: Parser::new(),
            screen: Screen::new(size.cols as usize, size.rows as usize, scrollback),
        })
    }

    /// Feeds child output through the parser, then writes any replies (cursor reports, DA) back to the child.
    pub fn feed(&mut self, bytes: &[u8]) {
        let _parse = flamewm_profiler::start("terminal.parse");
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
    #[must_use]
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

/// Spacing between UI wakes from one tab. Output inside the window is queued and wakes once when it ends.
pub const FRAME: Duration = Duration::from_millis(16);

/// Owns the wake socket and turns "output queued" signals into at most one wake per `FRAME`. A signal after idle
/// wakes at once, so a keystroke echo is not delayed; a signal inside the window sleeps to its end, then one wake
/// covers everything queued meanwhile, so trailing output is never stranded.
fn spawn_pacer(mut wake: UnixStream) -> Result<Sender<()>, String> {
    let (signal, queued) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("flamewm-terminal-pace".into())
        .spawn(move || {
            let mut last_wake: Option<Instant> = None;
            while queued.recv().is_ok() {
                if let Some(at) =
                    last_wake.and_then(|at| (at + FRAME).checked_duration_since(Instant::now()))
                {
                    std::thread::sleep(at);
                }
                // Everything the reader queued before this point is covered by the wake below.
                while queued.try_recv().is_ok() {}
                if wake.write_all(&[1]).is_err() {
                    return;
                }
                last_wake = Some(Instant::now());
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(signal)
}

fn read_loop(
    id: u64,
    mut reader: File,
    events: Sender<SessionEvent>,
    pacer: Sender<()>,
    notice_wake: UnixStream,
) {
    let _notice = ExitNotice {
        id,
        events: events.clone(),
        wake: notice_wake,
    };
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                if events
                    .send(SessionEvent::Output {
                        id,
                        bytes: buffer[..count].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
                if pacer.send(()).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // EIO is how Linux ends a PTY once the child's session is gone; any other error also ends the tab.
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use super::*;

    /// A flood of PTY output reaches the UI at frame cadence, not once per 16 KB read, and no byte is lost.
    #[test]
    fn a_flood_of_output_wakes_the_ui_at_frame_cadence_and_loses_no_bytes() {
        const FLOOD: usize = 4 * 1024 * 1024;
        let (events_tx, events_rx) = mpsc::channel();
        let (writer, mut peer) = UnixStream::pair().expect("socket pair");
        let launch = Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("head -c {FLOOD} /dev/zero")],
            cwd: None,
        };
        let size = PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        };
        let started = Instant::now();
        let _session = Session::start(1, &launch, size, 0, events_tx, writer).expect("start flood");
        let (mut bytes, mut chunks) = (0_usize, 0_usize);
        while let SessionEvent::Output { bytes: chunk, .. } = events_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("event before timeout")
        {
            bytes += chunk.len();
            chunks += 1;
        }
        let elapsed = started.elapsed();
        // Every wake is one byte on the socket. Exited's own wake may still be in flight, which only lowers the count.
        peer.set_nonblocking(true).expect("nonblocking peer");
        let mut wakes = 0_usize;
        let mut buffer = [0_u8; 64];
        loop {
            match peer.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => wakes += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("wake read: {error}"),
            }
        }
        assert_eq!(bytes, FLOOD, "every output byte is forwarded");
        assert!(chunks >= 16, "the flood arrives in many reads ({chunks})");
        let frames = elapsed.as_millis() as usize / FRAME.as_millis() as usize;
        assert!(
            wakes <= frames + 3,
            "{wakes} wakes in {elapsed:?} exceeds the frame cadence"
        );
        assert!(
            wakes * 4 <= chunks,
            "{wakes} wakes for {chunks} reads: no coalescing"
        );
    }
}
