//! One UI dispatch: parse the session output queued since the last wake, in arrival order, then commit one frame.
use std::io::Write;
use std::sync::mpsc::Receiver;

use crate::app::App;
use crate::session::SessionEvent;

/// Returned when the last tab closes. The run is a clean exit, not a failure.
pub const ALL_TABS_CLOSED: &str = "all tabs closed";

/// Output bytes parsed per dispatch. Past this, the rest stays queued and the loop is woken again, so a flood
/// (for example `cat` of a large file) cannot hold keyboard and pointer input behind it.
pub const OUTPUT_BUDGET: usize = 256 * 1024;

/// Parses queued session events in order until the queue is empty or `OUTPUT_BUDGET` bytes are parsed, then calls
/// `commit` once if anything changed. `changed` carries changes made outside the channel (settings). An empty
/// queue commits immediately, so a single keystroke echo is not delayed.
pub fn drain_and_commit(
    app: &mut App,
    events: &Receiver<SessionEvent>,
    changed: bool,
    commit: impl FnOnce(&mut App) -> Result<(), String>,
) -> Result<(), String> {
    let mut changed = changed;
    let mut parsed = 0;
    while parsed < OUTPUT_BUDGET {
        let Ok(event) = events.try_recv() else { break };
        changed = true;
        if let SessionEvent::Output { bytes, .. } = &event {
            parsed += bytes.len();
        }
        if app.session_event(event) {
            return Err(ALL_TABS_CLOSED.to_owned());
        }
    }
    if parsed >= OUTPUT_BUDGET {
        // The wake socket was drained before this call, so one byte cannot block; it re-dispatches the rest.
        let _ = app.wake.write(&[1]);
    }
    if changed { commit(app) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Sender};

    use super::*;
    use crate::pty::Launch;
    use crate::store::{TerminalConfig, ThemeStore};

    /// An App with one `cat` tab, a wake socket whose peer the test can read, and the session channel's sender.
    fn app_with_tab() -> (
        App,
        Sender<SessionEvent>,
        Receiver<SessionEvent>,
        UnixStream,
        u64,
    ) {
        let (events_tx, events_rx) = mpsc::channel();
        let (writer, peer) = UnixStream::pair().expect("socket pair");
        let mut app = App::new(
            TerminalConfig::default(),
            PathBuf::from("/nonexistent/flamewm-terminal/terminal.toml"),
            ThemeStore::new("/nonexistent/flamewm-terminal/themes"),
            None,
            PathBuf::from("/"),
            events_tx.clone(),
            writer,
            (640, 384),
            None,
        );
        app.refresh_metrics();
        app.set_window(640, 384);
        app.open_tab(Launch {
            program: "/bin/cat".into(),
            args: Vec::new(),
            cwd: None,
        })
        .expect("cat tab");
        let id = app.tabs[0].id;
        (app, events_tx, events_rx, peer, id)
    }

    /// The visible grid as text, rows joined without separators (rows are 80 columns, wrapped text fills them).
    fn screen_text(app: &App, id: u64) -> String {
        let screen = &app.pane(id).expect("pane").session.screen;
        (0..screen.rows())
            .map(|row| {
                screen
                    .line(row)
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn output(id: u64, bytes: &[u8]) -> SessionEvent {
        SessionEvent::Output {
            id,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn a_single_chunk_commits_one_frame_immediately() {
        let (mut app, tx, rx, _peer, id) = app_with_tab();
        tx.send(output(id, b"a")).unwrap();
        let frames = Cell::new(0);
        drain_and_commit(&mut app, &rx, false, |_| {
            frames.set(frames.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(frames.get(), 1);
        assert_eq!(screen_text(&app, id), "a");
        assert!(rx.try_recv().is_err(), "queue drained");
    }

    #[test]
    fn an_empty_queue_commits_nothing() {
        let (mut app, _tx, rx, _peer, _id) = app_with_tab();
        let frames = Cell::new(0);
        drain_and_commit(&mut app, &rx, false, |_| {
            frames.set(frames.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(frames.get(), 0);
    }

    #[test]
    fn a_hundred_chunks_in_one_batch_request_one_frame_and_keep_byte_order() {
        let (mut app, tx, rx, _peer, id) = app_with_tab();
        let alphabet: Vec<u8> = (0..100).map(|i| b'a' + (i % 26) as u8).collect();
        for byte in &alphabet {
            tx.send(output(id, &[*byte])).unwrap();
        }
        let frames = Cell::new(0);
        drain_and_commit(&mut app, &rx, false, |_| {
            frames.set(frames.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(frames.get(), 1, "one commit for the whole batch");
        let expected: String = alphabet.iter().map(|b| *b as char).collect();
        assert_eq!(screen_text(&app, id), expected);
    }

    #[test]
    fn output_past_the_budget_stays_queued_and_rearms_the_wake() {
        let (mut app, tx, rx, mut peer, id) = app_with_tab();
        let chunk = vec![b'x'; 64 * 1024];
        let chunks = OUTPUT_BUDGET / chunk.len() + 2;
        for _ in 0..chunks {
            tx.send(output(id, &chunk)).unwrap();
        }
        let frames = Cell::new(0);
        drain_and_commit(&mut app, &rx, false, |_| {
            frames.set(frames.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(frames.get(), 1);
        let remaining = std::iter::from_fn(|| rx.try_recv().ok()).count();
        assert!(
            remaining > 0 && remaining < chunks,
            "some output parsed, the rest queued: {remaining}"
        );
        let mut byte = [0_u8; 1];
        peer.read_exact(&mut byte).expect("wake re-armed");
    }

    #[test]
    fn settings_change_commits_without_output() {
        let (mut app, _tx, rx, _peer, _id) = app_with_tab();
        let frames = Cell::new(0);
        drain_and_commit(&mut app, &rx, true, |_| {
            frames.set(frames.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(frames.get(), 1);
    }
}
