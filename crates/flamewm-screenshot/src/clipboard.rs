//! Native X11 image/png clipboard ownership, including ICCCM INCR for large screenshots.
//! The owner blocks in WaitForEvent and exits when another application replaces it.

use std::path::Path;
use x11rb::NONE;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn atom<C: Connection>(conn: &C, name: &[u8]) -> Result<Atom, String> {
    conn.intern_atom(false, name)
        .map_err(err)?
        .reply()
        .map(|a| a.atom)
        .map_err(err)
}

#[derive(Clone, Copy)]
struct Atoms {
    selection: Atom,
    targets: Atom,
    mime: Atom,
    incr: Atom,
    timestamp: Atom,
}
impl Atoms {
    fn new<C: Connection>(conn: &C) -> Result<Self, String> {
        Ok(Self {
            selection: atom(conn, b"CLIPBOARD")?,
            targets: atom(conn, b"TARGETS")?,
            mime: atom(conn, b"image/png")?,
            incr: atom(conn, b"INCR")?,
            timestamp: atom(conn, b"TIMESTAMP")?,
        })
    }
}

#[derive(Clone, Copy)]
struct Transfer {
    window: u32,
    property: Atom,
    offset: usize,
}

/// `path` is read once and then no longer needed, so clipboard ownership is
/// independent of subsequent screenshot file deletion or replacement.
pub fn serve(path: &Path, temporary: bool) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(err)?;
    if temporary {
        let _ = std::fs::remove_file(path);
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("clipboard payload is not PNG".into());
    }
    let (conn, screen) = x11rb::connect(None).map_err(err)?;
    let atoms = Atoms::new(&conn)?;
    let root = conn.setup().roots[screen].root;
    let win = conn.generate_id().map_err(err)?;
    conn.create_window(
        0,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(err)?;
    // ICCCM TIMESTAMP must be the X server clock, not local wall time or CurrentTime=0.
    let tick = atom(&conn, b"_FLAMEWM_CLIPBOARD_TIME")?;
    conn.change_property8(PropMode::REPLACE, win, tick, AtomEnum::STRING, b"1")
        .map_err(err)?;
    conn.flush().map_err(err)?;
    let owner_time = loop {
        match conn.wait_for_event().map_err(err)? {
            Event::PropertyNotify(e) if e.window == win && e.atom == tick => break e.time,
            _ => {}
        }
    };
    conn.set_selection_owner(win, atoms.selection, owner_time)
        .map_err(err)?
        .check()
        .map_err(err)?;
    conn.flush().map_err(err)?;
    let owner = conn
        .get_selection_owner(atoms.selection)
        .map_err(err)?
        .reply()
        .map_err(err)?
        .owner;
    if owner != win {
        return Err("failed to claim clipboard".into());
    }
    let chunk = conn
        .maximum_request_bytes()
        .saturating_sub(128)
        .min(64 * 1024)
        .max(1024);
    let mut pending: Vec<Transfer> = Vec::new();
    loop {
        match conn.wait_for_event().map_err(err)? {
            Event::SelectionClear(e) if e.selection == atoms.selection => break,
            Event::DestroyNotify(e) => pending.retain(|p| p.window != e.window),
            Event::SelectionRequest(req) if req.selection == atoms.selection => {
                let property = if req.property == NONE {
                    req.target
                } else {
                    req.property
                };
                let mut selected = NONE;
                if req.target == atoms.targets {
                    conn.change_property32(
                        PropMode::REPLACE,
                        req.requestor,
                        property,
                        AtomEnum::ATOM,
                        &[atoms.targets, atoms.mime, atoms.timestamp],
                    )
                    .map_err(err)?;
                    selected = property;
                } else if req.target == atoms.timestamp {
                    conn.change_property32(
                        PropMode::REPLACE,
                        req.requestor,
                        property,
                        AtomEnum::INTEGER,
                        &[owner_time],
                    )
                    .map_err(err)?;
                    selected = property;
                } else if req.target == atoms.mime {
                    if bytes.len() <= chunk {
                        conn.change_property8(
                            PropMode::REPLACE,
                            req.requestor,
                            property,
                            atoms.mime,
                            &bytes,
                        )
                        .map_err(err)?;
                    } else {
                        // Subscribe before notifying requestor so its delete cannot race us.
                        conn.change_window_attributes(
                            req.requestor,
                            &ChangeWindowAttributesAux::new().event_mask(
                                EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY,
                            ),
                        )
                        .map_err(err)?;
                        conn.change_property32(
                            PropMode::REPLACE,
                            req.requestor,
                            property,
                            atoms.incr,
                            &[u32::try_from(bytes.len())
                                .map_err(|_| "PNG exceeds INCR protocol limit")?],
                        )
                        .map_err(err)?;
                        pending.push(Transfer {
                            window: req.requestor,
                            property,
                            offset: 0,
                        });
                    }
                    selected = property;
                }
                let event = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: req.time,
                    requestor: req.requestor,
                    selection: req.selection,
                    target: req.target,
                    property: selected,
                };
                conn.send_event(false, req.requestor, EventMask::NO_EVENT, event)
                    .map_err(err)?;
                conn.flush().map_err(err)?;
            }
            Event::PropertyNotify(note) if note.state == Property::DELETE => {
                if let Some(i) = pending
                    .iter()
                    .position(|p| p.window == note.window && p.property == note.atom)
                {
                    let entry = pending[i];
                    let end = (entry.offset + chunk).min(bytes.len());
                    conn.change_property8(
                        PropMode::REPLACE,
                        entry.window,
                        entry.property,
                        atoms.mime,
                        &bytes[entry.offset..end],
                    )
                    .map_err(err)?;
                    if entry.offset == bytes.len() {
                        pending.swap_remove(i);
                    } else {
                        pending[i].offset = end;
                    }
                    conn.flush().map_err(err)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Makes clipboard persistence optional: no resident process unless --copy is requested.
pub fn spawn_owner(path: &Path, temporary: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(err)?;
    let (conn, _) = x11rb::connect(None).map_err(err)?;
    let selection = atom(&conn, b"CLIPBOARD")?;
    let previous = conn
        .get_selection_owner(selection)
        .map_err(err)?
        .reply()
        .map_err(err)?
        .owner;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--serve-clipboard").arg(path);
    if temporary {
        cmd.arg("--temporary-clipboard");
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(err)?;
    // Avoid returning before the new owner actually takes CLIPBOARD.
    for _ in 0..100 {
        if child.try_wait().map_err(err)?.is_some() {
            return Err("clipboard owner exited before claiming selection".into());
        }
        let owner = conn
            .get_selection_owner(selection)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .owner;
        if owner != NONE && owner != previous {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("clipboard owner did not become ready".into())
}
