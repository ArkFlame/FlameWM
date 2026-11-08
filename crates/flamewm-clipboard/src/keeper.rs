//! Clipboard keeper: keeps the last CLIPBOARD text after its owner goes away.
//!
//! A background thread watches CLIPBOARD owner changes through `XFixes`. For each new owner it
//! reads text targets (`UTF8_STRING`, then `STRING`) and pushes them into a bounded
//! [`ClipHistory`]. Owners that advertise `x-kde-passwordManagerHint` in TARGETS, or that do
//! not answer TARGETS, are never kept. When the owner window or client closes, it serves the
//! newest kept text again through [`Clipboard::set_text`]. An explicit clear (`SetSelectionOwner`
//! with no owner) empties the history. Without `XFixes` the keeper reports once and ends.

use std::time::Instant;

use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEvent, SelectionEventMask};
use x11rb::protocol::xproto::{Atom, Window};

use crate::history::ClipHistory;
use crate::{Clipboard, GET_TIMEOUT, err, intern};

/// KDE/KeePassXC convention: owners advertising this target hold a password.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// Starts the keeper thread on `display` (or `$DISPLAY`). Returns once the thread is spawned.
pub fn spawn(display: Option<String>) -> Result<(), String> {
    std::thread::Builder::new()
        .name("flamewm-clipboard-keeper".into())
        .spawn(move || {
            if let Err(error) = run(display.as_deref()) {
                eprintln!("flamewm: clipboard keeper stopped: {error}");
            }
        })
        .map(|_| ())
        .map_err(err)
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// A new foreign owner: read its text (or clear the kept text when it has none).
    Fetch,
    /// The owner cleared the selection on purpose: forget the kept text.
    Forget,
    /// The owner window or client went away: serve the kept text again.
    Reserve,
}

fn step(owner: Window, subtype: SelectionEvent) -> Step {
    match (
        owner == NONE,
        u8::from(subtype) == u8::from(SelectionEvent::SET_SELECTION_OWNER),
    ) {
        (false, _) => Step::Fetch,
        (true, true) => Step::Forget,
        (true, false) => Step::Reserve,
    }
}

/// Whether an owner's text may be kept. `targets` is `None` when the owner did not answer
/// TARGETS; that and the password-manager hint both forbid keeping.
fn keepable(targets: Option<&[Atom]>, hint: Atom) -> bool {
    targets.is_some_and(|targets| !targets.contains(&hint))
}

/// Reads the atoms the CLIPBOARD owner advertises in TARGETS; `None` when it does not answer.
fn advertised(clipboard: &Clipboard, targets: Atom) -> Option<Vec<Atom>> {
    let requester = clipboard.requester.lock().ok()?;
    let bytes = requester
        .read(targets, Instant::now() + GET_TIMEOUT)
        .ok()??;
    Some(
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

fn run(display: Option<&str>) -> Result<(), String> {
    let (conn, screen) = x11rb::connect(display).map_err(err)?;
    conn.xfixes_query_version(5, 0)
        .map_err(err)?
        .reply()
        .map_err(err)?;
    let atoms = intern(&conn, &["CLIPBOARD", "TARGETS", PASSWORD_HINT])?;
    let (selection, targets, hint) = (atoms[0], atoms[1], atoms[2]);
    let root = conn.setup().roots[screen].root;
    conn.xfixes_select_selection_input(
        root,
        selection,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .map_err(err)?;
    conn.flush().map_err(err)?;
    let clipboard = Clipboard::connect_display(display)?;

    let mut history = ClipHistory::new();
    // True when the current owner's text is the newest history item and may be re-served.
    let mut live = false;
    loop {
        let Event::XfixesSelectionNotify(note) = conn.wait_for_event().map_err(err)? else {
            continue;
        };
        if note.selection != selection {
            continue;
        }
        match step(note.owner, note.subtype) {
            Step::Fetch => {
                let owner_targets = advertised(&clipboard, targets);
                let text = if keepable(owner_targets.as_deref(), hint) {
                    clipboard.get_text().ok().flatten()
                } else {
                    None
                };
                // A duplicate of the newest item is still the live text; a rejected item is not.
                live = text.is_some_and(|text| {
                    let same_as_newest = history.iter().next() == Some(text.as_str());
                    history.push(text) || same_as_newest
                });
            }
            Step::Forget => {
                history.clear();
                live = false;
            }
            Step::Reserve => {
                if let Some(text) = history.iter().next().filter(|_| live)
                    && let Err(error) = clipboard.set_text(text)
                {
                    eprintln!("flamewm: clipboard keeper could not re-serve text: {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_window_or_client_close_reserves_and_explicit_clear_forgets() {
        assert_eq!(
            step(0, SelectionEvent::SELECTION_WINDOW_DESTROY),
            Step::Reserve
        );
        assert_eq!(
            step(0, SelectionEvent::SELECTION_CLIENT_CLOSE),
            Step::Reserve
        );
        assert_eq!(step(0, SelectionEvent::SET_SELECTION_OWNER), Step::Forget);
    }

    #[test]
    fn foreign_owner_is_always_fetched() {
        assert_eq!(step(42, SelectionEvent::SET_SELECTION_OWNER), Step::Fetch);
        assert_eq!(
            step(42, SelectionEvent::SELECTION_WINDOW_DESTROY),
            Step::Fetch
        );
    }

    #[test]
    fn password_hint_or_unanswered_targets_are_never_kept() {
        const HINT: Atom = 9;
        assert!(keepable(Some(&[5, 6]), HINT));
        assert!(!keepable(Some(&[5, HINT]), HINT));
        assert!(!keepable(None, HINT));
    }
}
