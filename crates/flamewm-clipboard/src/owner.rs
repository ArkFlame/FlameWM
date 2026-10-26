//! Owner side: a background thread that holds CLIPBOARD and answers requests until
//! another client claims the selection (`SelectionClear`) or the connection dies.

use std::sync::{Arc, mpsc};

use x11rb::NONE;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::{err, intern};

const TICK_PROPERTY: &str = "_FLAMEWM_CLIPBOARD_TIME";
const MAX_CHUNK: usize = 64 * 1024;

/// Starts the owner thread and returns once it owns `selection` (CLIPBOARD or PRIMARY), or failed.
pub(crate) fn spawn(
    display: Option<String>,
    selection: &'static str,
    payloads: Vec<(&'static str, Vec<u8>)>,
) -> Result<(), String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("flamewm-clipboard-owner".into())
        .spawn(move || {
            // Failures after the claim have no caller left, so the thread just ends.
            if let Err(error) = run(display.as_deref(), selection, payloads, &ready_tx) {
                let _ = ready_tx.send(Err(error));
            }
        })
        .map_err(err)?;
    ready_rx.recv().map_err(err)?
}

/// Helper window for selection traffic: input-only, with `PropertyNotify` for ICCCM INCR.
pub(crate) fn helper_window(conn: &RustConnection, screen: usize) -> Result<u32, String> {
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().map_err(err)?;
    conn.create_window(
        0,
        window,
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
    Ok(window)
}

struct Served {
    targets: Atom,
    timestamp: Atom,
    incr: Atom,
    owner_time: u32,
    chunk: usize,
    payloads: Vec<(Atom, Arc<[u8]>)>,
}

/// An INCR transfer in progress: the next chunk goes out when the requestor deletes the property.
struct Transfer {
    window: u32,
    property: Atom,
    ty: Atom,
    data: Arc<[u8]>,
    offset: usize,
}

fn run(
    display: Option<&str>,
    selection_name: &'static str,
    payloads: Vec<(&'static str, Vec<u8>)>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let (conn, screen) = x11rb::connect(display).map_err(err)?;
    let mut names = vec![
        selection_name,
        "TARGETS",
        "TIMESTAMP",
        "INCR",
        TICK_PROPERTY,
    ];
    names.extend(payloads.iter().map(|(name, _)| *name));
    let atoms = intern(&conn, &names)?;
    let (selection, tick) = (atoms[0], atoms[4]);
    let window = helper_window(&conn, screen)?;

    // ICCCM requires a server timestamp; a PropertyNotify on our own window provides one.
    conn.change_property8(PropMode::REPLACE, window, tick, AtomEnum::STRING, b"1")
        .map_err(err)?;
    conn.flush().map_err(err)?;
    let owner_time = loop {
        if let Event::PropertyNotify(e) = conn.wait_for_event().map_err(err)?
            && e.window == window
            && e.atom == tick
        {
            break e.time;
        }
    };
    conn.set_selection_owner(window, selection, owner_time)
        .map_err(err)?
        .check()
        .map_err(err)?;
    conn.flush().map_err(err)?;
    let owner = conn
        .get_selection_owner(selection)
        .map_err(err)?
        .reply()
        .map_err(err)?
        .owner;
    if owner != window {
        return Err("another client kept CLIPBOARD".into());
    }
    ready.send(Ok(())).map_err(err)?;

    let served = Served {
        targets: atoms[1],
        timestamp: atoms[2],
        incr: atoms[3],
        owner_time,
        chunk: conn
            .maximum_request_bytes()
            .saturating_sub(128)
            .clamp(1024, MAX_CHUNK),
        payloads: atoms[5..]
            .iter()
            .zip(payloads)
            .map(|(atom, (_, bytes))| (*atom, Arc::from(bytes)))
            .collect(),
    };
    let mut pending = Vec::new();
    loop {
        match conn.wait_for_event().map_err(err)? {
            Event::SelectionClear(e) if e.selection == selection => return Ok(()),
            Event::SelectionRequest(req) if req.selection == selection => {
                answer(&conn, &served, &req, &mut pending)?;
                conn.flush().map_err(err)?;
            }
            Event::DestroyNotify(e) => pending.retain(|t| t.window != e.window),
            Event::PropertyNotify(note) if note.state == Property::DELETE => {
                send_next_chunk(&conn, served.chunk, &mut pending, note.window, note.atom)?;
            }
            _ => {}
        }
    }
}

fn answer(
    conn: &RustConnection,
    served: &Served,
    req: &SelectionRequestEvent,
    pending: &mut Vec<Transfer>,
) -> Result<(), String> {
    let property = if req.property == NONE {
        req.target
    } else {
        req.property
    };
    let mut selected = NONE;
    if req.target == served.targets {
        let mut list = vec![served.targets, served.timestamp];
        list.extend(served.payloads.iter().map(|(atom, _)| *atom));
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::ATOM,
            &list,
        )
        .map_err(err)?;
        selected = property;
    } else if req.target == served.timestamp {
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::INTEGER,
            &[served.owner_time],
        )
        .map_err(err)?;
        selected = property;
    } else if let Some((ty, data)) = served.payloads.iter().find(|(atom, _)| *atom == req.target) {
        if data.len() <= served.chunk {
            conn.change_property8(PropMode::REPLACE, req.requestor, property, *ty, data)
                .map_err(err)?;
        } else {
            // Subscribe before announcing INCR so the requestor's delete cannot race us.
            conn.change_window_attributes(
                req.requestor,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(err)?;
            let size = u32::try_from(data.len()).map_err(err)?;
            conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                property,
                served.incr,
                &[size],
            )
            .map_err(err)?;
            pending.push(Transfer {
                window: req.requestor,
                property,
                ty: *ty,
                data: Arc::clone(data),
                offset: 0,
            });
        }
        selected = property;
    }
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: selected,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)
        .map_err(err)?;
    Ok(())
}

/// Writes the next INCR chunk. A final empty write terminates the transfer.
fn send_next_chunk(
    conn: &RustConnection,
    chunk: usize,
    pending: &mut Vec<Transfer>,
    window: u32,
    property: Atom,
) -> Result<(), String> {
    let Some(i) = pending
        .iter()
        .position(|t| t.window == window && t.property == property)
    else {
        return Ok(());
    };
    let transfer = &mut pending[i];
    let start = transfer.offset;
    let end = (start + chunk).min(transfer.data.len());
    conn.change_property8(
        PropMode::REPLACE,
        transfer.window,
        transfer.property,
        transfer.ty,
        &transfer.data[start..end],
    )
    .map_err(err)?;
    conn.flush().map_err(err)?;
    transfer.offset = end;
    if start == transfer.data.len() {
        pending.swap_remove(i);
    }
    Ok(())
}
