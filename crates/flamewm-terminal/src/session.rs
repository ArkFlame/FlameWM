//! One tab: a PTY process, its VT parser and its screen. One reader thread per PTY forwards output to the UI loop and
//! wakes it through a socket pair, the same pattern flamewm-explorer uses for its workers. That same thread also owns
//! the frame pacing, so a tab is one thread and one wake source instead of a reader plus a pacer.
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

use crate::parser::Parser;
use crate::pty::{Launch, PtyProcess, PtySize};
use crate::screen::Screen;

/// What the reader threads report to the UI loop.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Output {
        id: u64,
        bytes: Vec<u8>,
    },
    Exited {
        id: u64,
    },
    /// One requested glide frame. Wakes the UI loop without touching a pane.
    Tick,
}

pub struct Session {
    pub id: u64,
    pub pty: PtyProcess,
    parser: Parser,
    pub screen: Screen,
    /// Cached line-discipline flags `(canonical, echoing)`, refreshed per output batch and per key press. The prompt
    /// check reads this instead of issuing an ioctl, so an idle repaint costs no syscall.
    modes: (bool, bool),
    /// Output buffers the UI has finished parsing, so the reader reuses an allocation instead of making one per event.
    recycle: Sender<Vec<u8>>,
    /// Output bytes sent to the UI and not yet parsed. The reader pauses while this is at the cap, so a
    /// flood the UI cannot keep up with fills the PTY (and blocks the child) instead of this process's memory.
    queued: Arc<AtomicUsize>,
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
        let (recycle, recycled) = mpsc::channel::<Vec<u8>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let reader_queued = Arc::clone(&queued);
        std::thread::Builder::new()
            .name(format!("flamewm-terminal-read-{id}"))
            .spawn(move || {
                read_loop(
                    id,
                    reader,
                    events,
                    wake,
                    notice_wake,
                    recycled,
                    reader_queued,
                );
            })
            .map_err(|error| error.to_string())?;
        let modes = pty.line_modes();
        Ok(Self {
            id,
            pty,
            parser: Parser::new(),
            screen: Screen::new(size.cols as usize, size.rows as usize, scrollback),
            modes,
            recycle,
            queued,
        })
    }

    /// Cached line-discipline flags `(canonical, echoing)`.
    #[must_use]
    pub fn line_modes(&self) -> (bool, bool) {
        self.modes
    }

    /// Reads the line-discipline flags once; call before a check that must see the child's newest state.
    pub fn refresh_line_modes(&mut self) {
        self.modes = self.pty.line_modes();
    }

    /// Feeds child output through the parser, then writes any replies (cursor reports, DA) back to the child.
    pub fn feed(&mut self, bytes: &[u8]) {
        let _parse = flamewm_profiler::start("terminal.parse");
        self.parser.advance(bytes, &mut self.screen);
        // A line editor toggles ICANON and ECHO around its prompt, always with output; sampling here keeps the cached
        // prompt check current without paying per frame. A flood batch skips the ioctl: a program emitting tens of KiB
        // at once is not sitting at a prompt, and the next small batch (the prompt echo) samples the pair again.
        if bytes.len() <= PROMPT_SAMPLE_BYTES {
            self.modes = self.pty.line_modes();
        }
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            let _ = self.pty.write(replies);
        }
    }

    /// Marks `len` output bytes as parsed, letting a reader paused at [`MAX_QUEUED_BYTES`] continue.
    pub fn consumed(&self, len: usize) {
        self.queued.fetch_sub(len, Ordering::Relaxed);
    }

    /// Returns an output buffer the UI finished parsing. The reader reuses it for the next batch, so a flood does not
    /// allocate a fresh batch buffer per event. Oversized buffers are dropped: the cap keeps memory bounded.
    pub fn recycle(&self, mut bytes: Vec<u8>) {
        const KEEP: usize = 2 * BATCH_BYTES;
        if bytes.capacity() <= KEEP {
            bytes.clear();
            // Unbounded channel: `send` only fails after the reader stopped, which is harmless here.
            let _ = self.recycle.send(bytes);
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

/// Bytes one `Output` event may carry. Data the kernel already holds is merged into one event, so a flood costs one
/// allocation, one channel message and one banner check per batch instead of one per few-KiB read.
const BATCH_BYTES: usize = 32 * 1024;
/// Output bytes allowed in flight to the UI before the reader pauses (about 16 ms of a fast flood).
const MAX_QUEUED_BYTES: usize = 4 * 1024 * 1024;

/// Batches at or below this size sample the line-discipline flags: smaller output is where a prompt echo arrives, and
/// a flood batch would otherwise pay an ioctl per 32 KiB just to read flags a running program is not changing.
const PROMPT_SAMPLE_BYTES: usize = 4096;

/// Reads the PTY and forwards batches to the UI loop, writing the wake byte at most once per `FRAME`.
/// The same thread does both jobs: when output is owed a wake, its `poll` timeout is the frame boundary,
/// so a tab keeps one parked thread instead of a reader plus a pacer and pays no second thread wake per
/// batch. A batch after an idle frame wakes at once, so a keystroke echo is never delayed.
fn read_loop(
    id: u64,
    mut reader: File,
    events: Sender<SessionEvent>,
    mut wake: UnixStream,
    notice_wake: UnixStream,
    recycled: Receiver<Vec<u8>>,
    queued: Arc<AtomicUsize>,
) {
    let _notice = ExitNotice {
        id,
        events: events.clone(),
        wake: notice_wake,
    };
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut batch = Vec::new();
    // Set while the UI is owed a wake; cleared when the byte is written.
    let mut unsent = false;
    let mut last_wake: Option<Instant> = None;
    loop {
        // The wake covers every batch queued before this pass, exactly as the pacer drained its queue.
        if unsent && last_wake.is_none_or(|at| Instant::now() >= at + FRAME) {
            if wake.write_all(&[1]).is_err() {
                return;
            }
            last_wake = Some(Instant::now());
            unsent = false;
        }
        // Backpressure: the UI has not parsed enough of what was already sent. Stop reading (the PTY
        // fills and the child blocks) and keep the frame-cadence wake above going until it catches up.
        if queued.load(Ordering::Relaxed) >= MAX_QUEUED_BYTES {
            std::thread::sleep(FRAME);
            continue;
        }
        let timeout = if unsent {
            last_wake.and_then(|at| (at + FRAME).checked_duration_since(Instant::now()))
        } else {
            // Nothing owed: block in `poll`, so an idle tab costs no wakeups.
            None
        };
        let timespec = timeout.map(|wait| Timespec {
            tv_sec: wait.as_secs() as _,
            tv_nsec: wait.subsec_nanos().into(),
        });
        let ready = {
            let fd = reader.as_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            poll(&mut fds, timespec.as_ref())
        };
        match ready {
            // Frame boundary reached: the next pass writes the wake byte.
            Ok(0) => continue,
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(_) => return,
        }
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                batch.extend_from_slice(&buffer[..count]);
                // Drain what the kernel already buffered into this same event: a flood would otherwise send one
                // event per few-KiB read, and each event costs an allocation, a channel message and a UI callback.
                while batch.len() < BATCH_BYTES {
                    let ready = rustix::io::ioctl_fionread(&reader).unwrap_or(0) as usize;
                    if ready == 0 {
                        break;
                    }
                    let want = ready.min(BATCH_BYTES - batch.len()).min(buffer.len());
                    match reader.read(&mut buffer[..want]) {
                        Ok(0) | Err(_) => break,
                        Ok(extra) => batch.extend_from_slice(&buffer[..extra]),
                    }
                }
                // Send the filled batch and take a buffer the UI handed back: a flood then reuses allocations instead
                // of growing a fresh `Vec` for every event. A missing recycled buffer keeps the old pre-sizing.
                let next = batch.len().min(BATCH_BYTES);
                let mut fresh = recycled.try_recv().unwrap_or_default();
                fresh.reserve(next);
                let bytes = std::mem::replace(&mut batch, fresh);
                queued.fetch_add(bytes.len(), Ordering::Relaxed);
                if events.send(SessionEvent::Output { id, bytes }).is_err() {
                    return;
                }
                unsent = true;
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

    /// A flood of PTY output reaches the UI as batched events at frame cadence, not once per read, and no byte is
    /// lost.
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
        let session = Session::start(1, &launch, size, 0, events_tx, writer).expect("start flood");
        let (mut bytes, mut chunks) = (0_usize, 0_usize);
        while let SessionEvent::Output { bytes: chunk, .. } = events_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("event before timeout")
        {
            bytes += chunk.len();
            // The real UI reports each parsed batch; without it the reader would pause at the cap.
            session.consumed(chunk.len());
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
        assert!(chunks >= 16, "the flood arrives in many events ({chunks})");
        let frames = elapsed.as_millis() as usize / FRAME.as_millis() as usize;
        assert!(
            wakes <= frames + 3,
            "{wakes} wakes in {elapsed:?} exceeds the frame cadence"
        );
        assert!(
            wakes * 2 <= chunks,
            "{wakes} wakes for {chunks} events: no coalescing"
        );
    }

    /// A flood the UI is not draining stops at the in-flight cap instead of growing without bound, and
    /// resumes once the UI reports the bytes parsed.
    #[test]
    fn an_undrained_flood_is_capped_and_resumes_when_the_ui_catches_up() {
        let (events_tx, events_rx) = mpsc::channel();
        let (writer, _peer) = UnixStream::pair().expect("socket pair");
        let launch = Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "yes".into()],
            cwd: None,
        };
        let size = PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        };
        let session = Session::start(1, &launch, size, 0, events_tx, writer).expect("start flood");
        // The reader fills toward the cap (32 KiB batches), then pauses.
        std::thread::sleep(Duration::from_secs(2));
        let before = session.queued.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        let after = session.queued.load(Ordering::Relaxed);
        assert!(
            after <= MAX_QUEUED_BYTES + BATCH_BYTES,
            "queued {after} bytes, cap {MAX_QUEUED_BYTES}"
        );
        assert_eq!(before, after, "a paused reader queues nothing more");
        // The UI drains and reports everything: the reader continues.
        let mut drained = 0_usize;
        while let Ok(SessionEvent::Output { bytes, .. }) = events_rx.try_recv() {
            drained += bytes.len();
            session.consumed(bytes.len());
        }
        assert!(drained > 0, "something was queued");
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            session.queued.load(Ordering::Relaxed) > 0,
            "output resumes after the UI catches up"
        );
    }
}
