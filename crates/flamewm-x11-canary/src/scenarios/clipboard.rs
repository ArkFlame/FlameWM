//! Cross-client clipboard canary: an owner and a requestor are two independent
//! X connections (neither is the canary's own connection). CLIPBOARD serves a
//! small UTF8_STRING payload; PRIMARY serves a payload above the INCR cap, so
//! its UTF8_STRING transfer runs the INCR property protocol with PropertyNotify
//! chunks. TARGETS is requested from both selections. The WM must never take
//! ownership of either selection during the exchange (no SelectionClear).
//!
//! Only selection-protocol messages (ConvertSelection, SelectionRequest,
//! SelectionNotify, PropertyNotify) are used; no input is synthesized.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::{CURRENT_TIME, NONE};

/// Payload above this size always takes the INCR path (independent of server limits).
const INCR_CHUNK_CAP: usize = 256 * 1024;
/// Owner-side payload for PRIMARY UTF8_STRING: forces several INCR chunks.
const BIG_PAYLOAD_LEN: usize = 600 * 1024;
const SMALL_TEXT: &str = "flamewm clipboard \u{2713} \u{e9}\n";
const WAIT: Duration = Duration::from_secs(10);

/// Deterministic non-repeating payload so truncation, reordering or gaps are detectable.
pub fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(131) ^ (i >> 7)) as u8)
        .collect()
}

/// INCR slice length: fits one ChangeProperty request and is 4-byte aligned so
/// GetProperty long_offset advances exactly.
pub fn incr_chunk_len(max_request_bytes: usize) -> usize {
    (max_request_bytes.saturating_sub(64) & !3).max(4)
}

/// Splits `data` into slices of at most `chunk` bytes; the terminating
/// zero-length chunk is sent separately by the owner.
#[cfg(test)]
pub fn incr_chunks(data: &[u8], chunk: usize) -> Vec<&[u8]> {
    data.chunks(chunk.max(1)).collect()
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    primary: Atom,
    targets: Atom,
    utf8: Atom,
    incr: Atom,
    data: Atom,
}

fn intern_atoms<C: Connection>(conn: &C) -> Result<Atoms, String> {
    let get = |name: &[u8]| -> Result<Atom, String> {
        Ok(conn
            .intern_atom(false, name)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .atom)
    };
    Ok(Atoms {
        clipboard: get(b"CLIPBOARD")?,
        primary: get(b"PRIMARY")?,
        targets: get(b"TARGETS")?,
        utf8: get(b"UTF8_STRING")?,
        incr: get(b"INCR")?,
        data: get(b"FLAMEWM_CLIP_DATA")?,
    })
}

fn create_window<C: Connection>(conn: &C, root: Window) -> Result<Window, String> {
    let wid = conn.generate_id().map_err(err)?;
    conn.create_window(
        0,
        wid,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .map_err(err)?
    .check()
    .map_err(err)?;
    Ok(wid)
}

#[derive(Debug, Default)]
struct OwnerReport {
    stolen: u32,
    served: u32,
    incr_transfers: u32,
}

/// Owner client: owns CLIPBOARD (small) and PRIMARY (big, INCR) until `stop`.
fn owner_main(
    atoms: Atoms,
    ready: mpsc::Sender<Result<Window, String>>,
    stop: Arc<AtomicBool>,
) -> Result<OwnerReport, String> {
    let (conn, screen) = x11rb::connect(None).map_err(err)?;
    let root = conn.setup().roots[screen].root;
    let wid = create_window(&conn, root)?;
    let chunk = incr_chunk_len(conn.maximum_request_bytes()).min(INCR_CHUNK_CAP);
    let big = pattern(BIG_PAYLOAD_LEN);
    for sel in [atoms.clipboard, atoms.primary] {
        conn.set_selection_owner(wid, sel, CURRENT_TIME)
            .map_err(err)?
            .check()
            .map_err(err)?;
    }
    conn.flush().map_err(err)?;
    let mut owned = true;
    for sel in [atoms.clipboard, atoms.primary] {
        let got = conn
            .get_selection_owner(sel)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .owner;
        owned &= got == wid;
    }
    if !owned {
        let _ = ready.send(Err("owner did not take CLIPBOARD+PRIMARY".to_owned()));
        return Err("owner did not take selections".to_owned());
    }
    let _ = ready.send(Ok(wid));

    let mut report = OwnerReport::default();
    // (requestor, property, bytes already sent) for in-flight INCR transfers.
    let mut pending: Vec<(Window, Atom, usize)> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let Some(event) = conn.poll_for_event().map_err(err)? else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        match event {
            Event::SelectionClear(clear)
                if clear.selection == atoms.clipboard || clear.selection == atoms.primary =>
            {
                report.stolen += 1;
            }
            Event::SelectionRequest(req) => {
                let prop = if req.property == NONE {
                    req.target
                } else {
                    req.property
                };
                let mut reply_prop = NONE;
                if req.target == atoms.targets {
                    conn.change_property32(
                        PropMode::REPLACE,
                        req.requestor,
                        prop,
                        AtomEnum::ATOM,
                        &[atoms.targets, atoms.utf8],
                    )
                    .map_err(err)?;
                    reply_prop = prop;
                } else if req.target == atoms.utf8 && req.selection == atoms.clipboard {
                    conn.change_property8(
                        PropMode::REPLACE,
                        req.requestor,
                        prop,
                        atoms.utf8,
                        SMALL_TEXT.as_bytes(),
                    )
                    .map_err(err)?;
                    reply_prop = prop;
                } else if req.target == atoms.utf8 && req.selection == atoms.primary {
                    let total = u32::try_from(big.len()).unwrap_or(u32::MAX);
                    conn.change_window_attributes(
                        req.requestor,
                        &ChangeWindowAttributesAux::new()
                            .event_mask(EventMask::PROPERTY_CHANGE),
                    )
                    .map_err(err)?;
                    conn.change_property32(
                        PropMode::REPLACE,
                        req.requestor,
                        prop,
                        atoms.incr,
                        &[total],
                    )
                    .map_err(err)?;
                    pending.push((req.requestor, prop, 0));
                    report.incr_transfers += 1;
                    reply_prop = prop;
                }
                let notify = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: req.time,
                    requestor: req.requestor,
                    selection: req.selection,
                    target: req.target,
                    property: reply_prop,
                };
                conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)
                    .map_err(err)?;
                conn.flush().map_err(err)?;
                if reply_prop != NONE && req.target != atoms.primary {
                    report.served += 1;
                }
            }
            Event::PropertyNotify(note)
                if note.state == Property::DELETE && note.atom == atoms.data =>
            {
                let Some(idx) = pending
                    .iter()
                    .position(|(w, p, _)| *w == note.window && *p == note.atom)
                else {
                    continue;
                };
                let offset = pending[idx].2;
                if offset < big.len() {
                    let end = (offset + chunk).min(big.len());
                    conn.change_property8(
                        PropMode::REPLACE,
                        note.window,
                        note.atom,
                        atoms.utf8,
                        &big[offset..end],
                    )
                    .map_err(err)?;
                    pending[idx].2 = end;
                } else {
                    conn.change_property8(
                        PropMode::REPLACE,
                        note.window,
                        note.atom,
                        atoms.utf8,
                        &[],
                    )
                    .map_err(err)?;
                    pending.swap_remove(idx);
                    report.served += 1;
                }
                conn.flush().map_err(err)?;
            }
            _ => {}
        }
    }
    let _ = conn.destroy_window(wid);
    let _ = conn.flush();
    Ok(report)
}

/// Waits for an event matching `pred` on the requestor connection.
fn wait_event<C: Connection, F: FnMut(&Event) -> bool>(
    conn: &C,
    mut pred: F,
) -> Result<Event, String> {
    let deadline = Instant::now() + WAIT;
    loop {
        while let Some(event) = conn.poll_for_event().map_err(err)? {
            if pred(&event) {
                return Ok(event);
            }
        }
        if Instant::now() >= deadline {
            return Err("timeout waiting for X event".to_owned());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Reads a property completely, looping on bytes_after. delete=true removes it
/// once fully read, which is what drives the INCR handshake.
fn read_property<C: Connection>(
    conn: &C,
    win: Window,
    prop: Atom,
) -> Result<(Atom, Vec<u8>), String> {
    let units = u32::try_from(conn.maximum_request_bytes() / 4)
        .unwrap_or(u32::MAX)
        .saturating_sub(64)
        .max(1);
    let mut value = Vec::new();
    let mut ty;
    loop {
        let reply = conn
            .get_property(
                true,
                win,
                prop,
                u32::from(AtomEnum::ANY),
                u32::try_from(value.len() / 4).map_err(err)?,
                units,
            )
            .map_err(err)?
            .reply()
            .map_err(err)?;
        ty = reply.type_;
        value.extend_from_slice(&reply.value);
        if reply.bytes_after == 0 {
            break;
        }
    }
    Ok((ty, value))
}

/// Result of one owner/requestor exchange.
struct CaseResult {
    name: &'static str,
    pass: bool,
    detail: String,
}

fn run_case<C: Connection>(
    conn: &C,
    atoms: Atoms,
    wid: Window,
    name: &'static str,
    selection: Atom,
    target: Atom,
    expected: &[u8],
    expect_incr: bool,
) -> CaseResult {
    let fail = |detail: String| CaseResult {
        name,
        pass: false,
        detail,
    };
    if let Err(e) = conn
        .convert_selection(wid, selection, target, atoms.data, CURRENT_TIME)
        .map_err(err)
        .and_then(|c| c.check().map_err(err))
        .and_then(|()| conn.flush().map_err(err))
    {
        return fail(format!("convert failed: {e}"));
    }
    let notify = match wait_event(conn, |e| {
        matches!(e, Event::SelectionNotify(n) if n.requestor == wid && n.selection == selection && n.target == target)
    }) {
        Ok(Event::SelectionNotify(n)) => n,
        Ok(_) => return fail("unexpected event".to_owned()),
        Err(e) => return fail(e),
    };
    if notify.property == NONE {
        return fail("owner refused conversion (property=None)".to_owned());
    }
    let (ty, mut data) = match read_property(conn, wid, atoms.data) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let incr = ty == atoms.incr;
    if incr {
        let total = data
            .get(..4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, u32::from_ne_bytes);
        data.clear();
        loop {
            let event = wait_event(conn, |e| {
                matches!(e, Event::PropertyNotify(p) if p.window == wid && p.atom == atoms.data && p.state == Property::NEW_VALUE)
            });
            if let Err(e) = event {
                return fail(format!("INCR chunk timeout after {} bytes: {e}", data.len()));
            }
            let (_, chunk) = match read_property(conn, wid, atoms.data) {
                Ok(v) => v,
                Err(e) => return fail(e),
            };
            if chunk.is_empty() {
                break;
            }
            data.extend_from_slice(&chunk);
        }
        if data.len() != total as usize {
            return fail(format!("INCR size announced={total} got={}", data.len()));
        }
    }
    let identical = data == expected;
    let pass = identical && incr == expect_incr;
    CaseResult {
        name,
        pass,
        detail: format!(
            "bytes={} expected={} incr={incr} identical={identical}",
            data.len(),
            expected.len()
        ),
    }
}

/// Run the cross-client clipboard scenario (explicit-only; needs the live WM).
pub fn run(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let _ = args;
    let atoms = match intern_atoms(&canary.conn) {
        Ok(a) => a,
        Err(e) => return (false, format!("scenario=clipboard atoms failed: {e}")),
    };
    let (stop, owner_handle, ready_rx) = {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_t = Arc::clone(&stop);
        let handle = thread::spawn(move || owner_main(atoms, tx, stop_t));
        (stop, handle, rx)
    };
    let owner_wid = match ready_rx.recv_timeout(WAIT) {
        Ok(Ok(wid)) => wid,
        Ok(Err(e)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = owner_handle.join();
            return (false, format!("scenario=clipboard owner setup failed: {e}"));
        }
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            return (false, format!("scenario=clipboard owner not ready: {e}"));
        }
    };
    let outcome = requestor_cases(canary.root, atoms);
    let owners_before_stop = [atoms.clipboard, atoms.primary].map(|sel| {
        canary
            .conn
            .get_selection_owner(sel)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(NONE, |r| r.owner)
    });
    stop.store(true, Ordering::Relaxed);
    let report = owner_handle.join();
    let (report, owner_err) = match report {
        Ok(Ok(r)) => (r, None),
        Ok(Err(e)) => (OwnerReport::default(), Some(e)),
        Err(_) => (OwnerReport::default(), Some("owner thread panicked".to_owned())),
    };
    let owners_held = owners_before_stop == [owner_wid, owner_wid];
    let (cases, setup_err) = match outcome {
        Ok(c) => (c, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let all_cases = !cases.is_empty() && cases.iter().all(|c| c.pass);
    let pass = all_cases
        && setup_err.is_none()
        && owner_err.is_none()
        && report.stolen == 0
        && owners_held
        && report.incr_transfers >= 1;
    let case_text: Vec<String> = cases
        .iter()
        .map(|c| format!("{}={} ({})", c.name, if c.pass { "PASS" } else { "FAIL" }, c.detail))
        .collect();
    (
        pass,
        format!(
            "scenario=clipboard path=two-independent-x11-clients owner_window={owner_wid} stolen={} owners_held={owners_held} incr_transfers={} cases=[{}] setup_err={:?} owner_err={:?} send_event=selection-protocol-only input=none",
            report.stolen,
            report.incr_transfers,
            case_text.join("; "),
            setup_err,
            owner_err,
        ),
    )
}

fn requestor_cases(root: Window, atoms: Atoms) -> Result<Vec<CaseResult>, String> {
    let (conn, _) = x11rb::connect(None).map_err(err)?;
    let wid = create_window(&conn, root)?;
    conn.change_window_attributes(
        wid,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(err)?
    .check()
    .map_err(err)?;
    let mut targets_expected = Vec::new();
    for atom in [atoms.targets, atoms.utf8] {
        targets_expected.extend_from_slice(&atom.to_ne_bytes());
    }
    let small = SMALL_TEXT.as_bytes().to_vec();
    let big = pattern(BIG_PAYLOAD_LEN);
    let cases = [
        ("clipboard-targets", atoms.clipboard, atoms.targets, targets_expected.clone(), false),
        ("clipboard-utf8", atoms.clipboard, atoms.utf8, small, false),
        ("primary-targets", atoms.primary, atoms.targets, targets_expected, false),
        ("primary-utf8-incr", atoms.primary, atoms.utf8, big, true),
    ];
    let mut results = Vec::new();
    for (name, sel, target, expected, expect_incr) in cases {
        results.push(run_case(&conn, atoms, wid, name, sel, target, &expected, expect_incr));
    }
    let _ = conn.destroy_window(wid);
    let _ = conn.flush();
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incr_chunks_reassemble_byte_identical() {
        let data = pattern(BIG_PAYLOAD_LEN);
        let chunk = incr_chunk_len(262_144).min(INCR_CHUNK_CAP);
        let parts = incr_chunks(&data, chunk);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.len() <= chunk));
        assert_eq!(parts.concat(), data);
    }

    #[test]
    fn incr_chunk_len_is_aligned_and_fits_request() {
        let len = incr_chunk_len(262_144);
        assert_eq!(len % 4, 0);
        assert!(len + 64 <= 262_144);
        assert_eq!(incr_chunk_len(0), 4);
    }

    #[test]
    fn small_payload_is_single_chunk() {
        let data = SMALL_TEXT.as_bytes();
        let parts = incr_chunks(data, 4096);
        assert_eq!(parts, vec![data]);
        assert!(incr_chunks(&[], 4096).is_empty());
    }

    #[test]
    fn pattern_is_deterministic_and_not_periodic_at_chunk_edges() {
        let a = pattern(BIG_PAYLOAD_LEN);
        assert_eq!(a.len(), BIG_PAYLOAD_LEN);
        assert_eq!(a, pattern(BIG_PAYLOAD_LEN));
        assert_ne!(a[..4096], a[4096..8192]);
    }
}
