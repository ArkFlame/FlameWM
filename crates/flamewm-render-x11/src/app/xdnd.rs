//! X11 side of XDND (protocol v5): atom and message I/O, the drop-target handlers, and the
//! modal file-drag source loop. Protocol decisions live in `crate::xdnd`.
//!
//! Only windows created with `X11Config::file_drag` take part. Drag-source selection
//! requests are answered inside the modal loop; drop-target data arrives as `SelectionNotify`
//! on the normal event loop.

use std::mem::MaybeUninit;
use std::os::raw::{c_int, c_long, c_uint};
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

use flamewm_render_core::{ActionEvent, ActionPhase, RuntimeDocument};

use super::*;
use crate::config::{
    FILE_DRAG_DROP_ACTION, FILE_DRAG_ENTER_ACTION, FILE_DRAG_EXTRACT_ACTION,
    FILE_DRAG_LEAVE_ACTION, FILE_DRAG_OVER_ACTION, FileDragSlot,
};
use crate::xdnd::{self as protocol, DropEvent, DropTarget, FormatAtoms, Reply};
use crate::xlib_ext::{
    XConvertSelection, XGetSelectionOwner, XGetWindowProperty, XSendEvent, XSetSelectionOwner,
    XTranslateCoordinates,
};

const SELECTION_CLEAR: c_int = 29;
const SELECTION_REQUEST: c_int = 30;
pub(crate) const SELECTION_NOTIFY: c_int = 31;
const QUEUED_AFTER_FLUSH: c_int = 2;
const GRAB_SUCCESS: c_int = 0;
const FINISH_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_WINDOW_DEPTH: usize = 64;
/// Upper bound for one property read (32-bit units) and for one served `text/uri-list`.
const MAX_PROPERTY_UNITS: c_long = 1 << 22;
const MAX_SERVED_BYTES: usize = 256 * 1024;
const DATA_PROPERTY: &str = "FLAMEWM_XDND_DATA";

#[derive(Clone, Copy)]
pub(crate) struct XdndAtoms {
    aware: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop_: Atom,
    finished: Atom,
    pub(crate) selection: Atom,
    action_copy: Atom,
    type_list: Atom,
    uri_list: Atom,
    text_plain: Atom,
    ark_service: Atom,
    ark_path: Atom,
    targets: Atom,
    data: Atom,
}

impl XdndAtoms {
    fn intern(display: *mut Display) -> Self {
        let get = |name: &str| intern_atom(display, name);
        Self {
            aware: get("XdndAware"),
            enter: get("XdndEnter"),
            position: get("XdndPosition"),
            status: get("XdndStatus"),
            leave: get("XdndLeave"),
            drop_: get("XdndDrop"),
            finished: get("XdndFinished"),
            selection: get("XdndSelection"),
            action_copy: get("XdndActionCopy"),
            type_list: get("XdndTypeList"),
            uri_list: get(protocol::URI_LIST),
            text_plain: get(protocol::TEXT_PLAIN),
            ark_service: get(protocol::ARK_SERVICE_TYPE),
            ark_path: get(protocol::ARK_PATH_TYPE),
            targets: get("TARGETS"),
            data: get(DATA_PROPERTY),
        }
    }
}

/// Per-window XDND state. Created only when the window opts in.
pub(crate) struct XdndState {
    pub(crate) atoms: XdndAtoms,
    pub(crate) file_drag: FileDragSlot,
    target: DropTarget,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SelectionEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: Time,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SelectionRequestEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    owner: Window,
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: Time,
}

impl XdndState {
    /// Advertises `XdndAware` (protocol v5) on `window` and returns the window's XDND state.
    ///
    /// # Safety
    ///
    /// `display` and `window` must be live and owned by the caller.
    pub(crate) unsafe fn setup(
        display: *mut Display,
        window: Window,
        file_drag: FileDragSlot,
    ) -> Self {
        let atoms = XdndAtoms::intern(display);
        let version: [c_long; 1] = [protocol::XDND_VERSION as c_long];
        // SAFETY: the buffer holds one 32-bit property unit, matching format 32 (c_long items).
        unsafe {
            XChangeProperty(
                display,
                window,
                atoms.aware,
                XA_ATOM,
                32,
                PROP_MODE_REPLACE,
                version.as_ptr().cast(),
                1,
            );
        }
        Self {
            atoms,
            file_drag,
            target: DropTarget::default(),
        }
    }
}

fn longs(bytes: &[u8]) -> Vec<u64> {
    const WIDTH: usize = std::mem::size_of::<c_long>();
    bytes
        .chunks_exact(WIDTH)
        .map(|chunk| {
            let mut raw = [0u8; WIDTH];
            raw.copy_from_slice(chunk);
            c_long::from_ne_bytes(raw) as u64
        })
        .collect()
}

/// Reads a property, optionally deleting it. Returns `(type, format, bytes)`.
///
/// # Safety
///
/// `display` must be live and `window` must be a valid window id.
unsafe fn read_property(
    display: *mut Display,
    window: Window,
    property: Atom,
    delete: bool,
) -> Option<(Atom, c_int, Vec<u8>)> {
    let mut actual_type: Atom = 0;
    let mut actual_format: c_int = 0;
    let mut items: c_ulong = 0;
    let mut after: c_ulong = 0;
    let mut data: *mut u8 = ptr::null_mut();
    // SAFETY: all out-pointers reference live locals; Xlib allocates `data`, freed below.
    unsafe {
        let status = XGetWindowProperty(
            display,
            window,
            property,
            0,
            MAX_PROPERTY_UNITS,
            delete as Bool,
            0,
            &mut actual_type,
            &mut actual_format,
            &mut items,
            &mut after,
            &mut data,
        );
        if status != 0 || data.is_null() {
            if !data.is_null() {
                XFree(data.cast());
            }
            return None;
        }
        let unit = match actual_format {
            8 => 1,
            16 => 2,
            32 => std::mem::size_of::<c_long>(),
            _ => 0,
        };
        let bytes = if unit == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data, items as usize * unit).to_vec()
        };
        XFree(data.cast());
        Some((actual_type, actual_format, bytes))
    }
}

/// # Safety
///
/// `display` must be live and `window` a valid window id.
unsafe fn read_atoms(display: *mut Display, window: Window, property: Atom) -> Vec<u64> {
    // SAFETY: forwarded from the caller.
    match unsafe { read_property(display, window, property, false) } {
        Some((_, 32, bytes)) => longs(&bytes),
        _ => Vec::new(),
    }
}

/// Sends a ClientMessage (format 32) with `window` in data[0] and four payload longs after it.
///
/// # Safety
///
/// `display` must be live; `to` and `window` must be valid window ids.
unsafe fn send_client(
    display: *mut Display,
    to: Window,
    message: Atom,
    window: Window,
    payload: [c_long; 4],
) {
    // SAFETY: a zeroed XEvent is a valid all-zero union; the client-message arm is then written.
    unsafe {
        let mut event: XEvent = std::mem::zeroed();
        event.xclient = XClientMessageEvent {
            type_: CLIENT_MESSAGE,
            serial: 0,
            send_event: 1,
            display,
            window: to,
            message_type: message,
            format: 32,
            data: ClientMessageData {
                l: [
                    window as c_long,
                    payload[0],
                    payload[1],
                    payload[2],
                    payload[3],
                ],
            },
        };
        XSendEvent(display, to, 0, 0, &mut event);
        XFlush(display);
    }
}

/// # Safety
///
/// `display` must be live; `requestor` must be a valid window id.
unsafe fn send_selection_notify(
    display: *mut Display,
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: Time,
) {
    // SAFETY: XEvent is at least as large as SelectionEvent and both are plain-old-data.
    unsafe {
        let mut event: XEvent = std::mem::zeroed();
        ptr::write(
            &mut event as *mut XEvent as *mut SelectionEvent,
            SelectionEvent {
                type_: SELECTION_NOTIFY,
                serial: 0,
                send_event: 1,
                display,
                requestor,
                selection,
                target,
                property,
                time,
            },
        );
        XSendEvent(display, requestor, 0, 0, &mut event);
        XFlush(display);
    }
}

fn drag_action_event(event: DropEvent) -> ActionEvent {
    let (action, x, y, text) = match event {
        DropEvent::Enter { x, y } => (FILE_DRAG_ENTER_ACTION, x, y, None),
        DropEvent::Over { x, y } => (FILE_DRAG_OVER_ACTION, x, y, None),
        DropEvent::Leave => (FILE_DRAG_LEAVE_ACTION, 0, 0, None),
        DropEvent::Extract {
            x,
            y,
            service,
            path,
        } => (
            FILE_DRAG_EXTRACT_ACTION,
            x,
            y,
            Some(format!("{service}\u{1f}{path}")),
        ),
        DropEvent::Drop { x, y, paths } => (
            FILE_DRAG_DROP_ACTION,
            x,
            y,
            Some(String::from_utf8_lossy(&protocol::encode_uri_list(&paths)).into_owned()),
        ),
    };
    ActionEvent {
        action: action.to_string(),
        phase: ActionPhase::Release,
        button: 0,
        modifiers: 0,
        x: x as f32,
        y: y as f32,
        inside: true,
        time_ms: 0,
        text,
    }
}

fn emit_drag_events<F>(
    events: Vec<DropEvent>,
    document: &mut RuntimeDocument,
    on_action: &mut F,
) -> Result<(), String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    for event in events {
        on_action(&drag_action_event(event), document)?;
    }
    Ok(())
}

impl X11App {
    /// Takes an outgoing file-drag request left by the UI. Errors are logged, never fatal.
    ///
    /// # Safety
    ///
    /// Must run on the thread that owns the live display.
    pub(crate) unsafe fn start_pending_file_drag(&mut self) {
        let Some(paths) = self
            .xdnd
            .as_ref()
            .and_then(|state| state.file_drag.borrow_mut().take())
        else {
            return;
        };
        // SAFETY: forwarded from the caller.
        if let Err(error) = unsafe { self.run_file_drag(&paths) } {
            eprintln!("FLAMEWM_XDND_DRAG_FAILED reason={error}");
        }
    }

    /// `handle_xdnd_message` with panics contained: a panic resets the XDND session and the event loop
    /// continues. Errors and `Ok` results pass through unchanged.
    ///
    /// # Safety
    ///
    /// Must run on the thread that owns the live display.
    pub(crate) unsafe fn handle_xdnd_message<F>(
        &mut self,
        client: &XClientMessageEvent,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: forwarded from the caller.
            unsafe { self.handle_xdnd_message_inner(client, document, on_action) }
        }));
        outcome.unwrap_or_else(|_| {
            eprintln!("FLAMEWM_XDND_PANIC contained in handle_xdnd_message; drop session reset");
            if let Some(state) = self.xdnd.as_mut() {
                state.target = DropTarget::default();
            }
            Ok(true)
        })
    }

    /// Handles one XDND ClientMessage addressed to this window. Returns `false` for other messages.
    ///
    /// # Safety
    ///
    /// Must run on the thread that owns the live display.
    unsafe fn handle_xdnd_message_inner<F>(
        &mut self,
        client: &XClientMessageEvent,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let Some(atoms) = self.xdnd.as_ref().map(|state| state.atoms) else {
            return Ok(false);
        };
        // SAFETY: the client message was produced with format 32, so `l` is the live union arm.
        let data = unsafe { client.data.l };
        let source = data[0] as u64;
        let kind = client.message_type;
        if kind == atoms.enter {
            let enter = protocol::decode_enter([data[0], data[1], data[2], data[3], data[4]]);
            // SAFETY: the source window is named by the message; reads are guarded by Xlib errors.
            let offered = if enter.more_types {
                unsafe { read_atoms(self.display, enter.source as Window, atoms.type_list) }
            } else {
                enter.types.clone()
            };
            let formats = FormatAtoms {
                uri_list: atoms.uri_list as u64,
                ark_service: atoms.ark_service as u64,
                ark_path: atoms.ark_path as u64,
                text_plain: atoms.text_plain as u64,
            };
            let offer = protocol::choose_offer(&offered, &formats);
            if let Some(state) = self.xdnd.as_mut() {
                state.target.enter_offer(&enter, offer);
            }
            Ok(true)
        } else if kind == atoms.position {
            let (root_x, root_y) = protocol::unpack_position(data[2]);
            // SAFETY: display and window are this app's live resources.
            let (x, y) = unsafe { self.root_to_window(root_x, root_y) };
            let Some(state) = self.xdnd.as_mut() else {
                return Ok(true);
            };
            let (events, replies) = state.target.position(source, x, y);
            // SAFETY: replies reference this window's live display.
            unsafe { self.send_replies(&replies, atoms) };
            emit_drag_events(events, document, on_action)?;
            Ok(true)
        } else if kind == atoms.leave {
            let events = self
                .xdnd
                .as_mut()
                .map(|state| state.target.leave(source))
                .unwrap_or_default();
            emit_drag_events(events, document, on_action)?;
            Ok(true)
        } else if kind == atoms.drop_ {
            let time = data[2] as u64;
            let replies = match self.xdnd.as_mut() {
                Some(state) => state.target.drop_requested(source, time),
                None => return Ok(true),
            };
            // SAFETY: replies reference this window's live display.
            unsafe { self.send_replies(&replies, atoms) };
            Ok(true)
        } else {
            // Stray status/finished replies belong to a drag this window is not sourcing.
            Ok(kind == atoms.status || kind == atoms.finished)
        }
    }

    /// `handle_xdnd_selection_notify` with panics contained: a panic resets the XDND session and the event loop
    /// continues. Errors and `Ok` results pass through unchanged.
    ///
    /// # Safety
    ///
    /// `event` must be a SelectionNotify XEvent read from this app's display.
    pub(crate) unsafe fn handle_xdnd_selection_notify<F>(
        &mut self,
        event: &XEvent,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: forwarded from the caller.
            unsafe { self.handle_xdnd_selection_notify_inner(event, document, on_action) }
        }));
        outcome.unwrap_or_else(|_| {
            eprintln!(
                "FLAMEWM_XDND_PANIC contained in handle_xdnd_selection_notify; drop session reset"
            );
            if let Some(state) = self.xdnd.as_mut() {
                state.target = DropTarget::default();
            }
            Ok(true)
        })
    }

    /// Completes a pending drop when the converted `XdndSelection` arrives.
    ///
    /// # Safety
    ///
    /// `event` must be a SelectionNotify XEvent read from this app's display.
    unsafe fn handle_xdnd_selection_notify_inner<F>(
        &mut self,
        event: &XEvent,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let Some(atoms) = self.xdnd.as_ref().map(|state| state.atoms) else {
            return Ok(false);
        };
        // SAFETY: SelectionNotify shares SelectionEvent's layout; XEvent is at least that large.
        let note = unsafe { ptr::read(event as *const XEvent as *const SelectionEvent) };
        if note.requestor != self.window || note.selection != atoms.selection {
            return Ok(false);
        }
        // SAFETY: the window is this app's; the property is consumed (deleted) once read.
        let bytes = if note.property == 0 {
            None
        } else {
            unsafe { read_property(self.display, self.window, atoms.data, true) }
                .map(|(_, _, bytes)| bytes)
        };
        let Some(state) = self.xdnd.as_mut() else {
            return Ok(false);
        };
        let (events, replies) = state.target.selection_bytes(bytes);
        // SAFETY: replies reference this window's live display.
        unsafe { self.send_replies(&replies, atoms) };
        emit_drag_events(events, document, on_action)?;
        Ok(true)
    }

    /// # Safety
    ///
    /// Display and window must be this app's live resources.
    unsafe fn send_replies(&self, replies: &[Reply], atoms: XdndAtoms) {
        for reply in replies {
            match *reply {
                Reply::Status { source, accepted } => unsafe {
                    send_client(
                        self.display,
                        source as Window,
                        atoms.status,
                        self.window,
                        [
                            protocol::status_flags(accepted) as c_long,
                            0,
                            0,
                            if accepted {
                                atoms.action_copy as c_long
                            } else {
                                0
                            },
                        ],
                    );
                },
                Reply::Finished { source, accepted } => unsafe {
                    send_client(
                        self.display,
                        source as Window,
                        atoms.finished,
                        self.window,
                        [
                            protocol::status_flags(accepted) as c_long,
                            if accepted {
                                atoms.action_copy as c_long
                            } else {
                                0
                            },
                            0,
                            0,
                        ],
                    );
                },
                Reply::Convert { format, time } => unsafe {
                    XConvertSelection(
                        self.display,
                        atoms.selection,
                        format as Atom,
                        atoms.data,
                        self.window,
                        time as Time,
                    );
                    XFlush(self.display);
                },
            }
        }
    }

    /// Maps a root-relative pointer position into this window's coordinates.
    ///
    /// # Safety
    ///
    /// Display and window must be this app's live resources.
    unsafe fn root_to_window(&self, root_x: i32, root_y: i32) -> (i32, i32) {
        // SAFETY: display is live; out-pointers reference locals.
        unsafe {
            let root = XRootWindow(self.display, self.screen);
            let (mut x, mut y, mut child) = (0, 0, 0 as Window);
            XTranslateCoordinates(
                self.display,
                root,
                self.window,
                root_x,
                root_y,
                &mut x,
                &mut y,
                &mut child,
            );
            (x, y)
        }
    }

    /// Finds the deepest XdndAware window under a root point (excluding this window).
    ///
    /// # Safety
    ///
    /// Display must be live.
    unsafe fn xdnd_target_at(
        &self,
        x_root: i32,
        y_root: i32,
        aware: Atom,
    ) -> Option<(Window, u64)> {
        // SAFETY: display is live; out-pointers reference locals.
        unsafe {
            let root = XRootWindow(self.display, self.screen);
            let mut chain: Vec<Window> = Vec::new();
            let mut parent = root;
            while chain.len() < MAX_WINDOW_DEPTH {
                let (mut x, mut y, mut child) = (0, 0, 0 as Window);
                XTranslateCoordinates(
                    self.display,
                    root,
                    parent,
                    x_root,
                    y_root,
                    &mut x,
                    &mut y,
                    &mut child,
                );
                if child == 0 {
                    break;
                }
                chain.push(child);
                parent = child;
            }
            chain
                .iter()
                .rev()
                .filter(|window| **window != self.window)
                .find_map(|&window| {
                    read_property(self.display, window, aware, false)
                        .and_then(|(_, format, bytes)| {
                            (format == 32)
                                .then(|| longs(&bytes).first().copied())
                                .flatten()
                        })
                        .map(|version| (window, version))
                })
        }
    }

    /// Runs a modal XDND drag of `paths` from this window. Blocks until the drop finishes,
    /// the drag is cancelled, or the pointer is released over nothing.
    ///
    /// # Safety
    ///
    /// Must run on the thread that owns the live display.
    pub(crate) unsafe fn run_file_drag(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let atoms = self
            .xdnd
            .as_ref()
            .map(|state| state.atoms)
            .ok_or("file drag requires an XDND-enabled window")?;
        if paths.is_empty() || !paths.iter().all(|path| path.is_absolute()) {
            return Err("file drag needs a non-empty list of absolute paths".into());
        }
        let (display, window) = (self.display, self.window);
        // SAFETY: display and window are live; every out-pointer is a local.
        unsafe {
            let root = XRootWindow(display, self.screen);
            XSetSelectionOwner(display, atoms.selection, window, CURRENT_TIME);
            if XGetSelectionOwner(display, atoms.selection) != window {
                return Err("could not own XdndSelection".into());
            }
            let grabbed = XGrabPointer(
                display,
                root,
                0,
                (POINTER_MOTION_MASK | BUTTON_RELEASE_MASK) as c_uint,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                0,
                0,
                CURRENT_TIME,
            );
            let mut deferred = Vec::new();
            let result = if grabbed == GRAB_SUCCESS {
                self.drag_loop(atoms, paths, &mut deferred)
            } else {
                Err(format!("pointer grab failed status={grabbed}"))
            };
            XUngrabPointer(display, CURRENT_TIME);
            if XGetSelectionOwner(display, atoms.selection) == window {
                XSetSelectionOwner(display, atoms.selection, 0, CURRENT_TIME);
            }
            // Events this loop does not own go back to the window loop in their original order.
            for event in deferred.iter_mut().rev() {
                XPutBackEvent(display, event);
            }
            XFlush(display);
            result
        }
    }

    /// # Safety
    ///
    /// Must run while this app owns the display and XdndSelection.
    unsafe fn drag_loop(
        &self,
        atoms: XdndAtoms,
        paths: &[PathBuf],
        deferred: &mut Vec<XEvent>,
    ) -> Result<(), String> {
        let (display, window) = (self.display, self.window);
        // Target window and whether it last accepted the position.
        let mut target: Option<(Window, bool)> = None;
        let mut last_position: Option<(c_int, c_int)> = None;
        let mut time: Time;
        let mut dropped: Option<(Window, Instant)> = None;
        let uri = atoms.uri_list as c_long;
        let plain = atoms.text_plain as c_long;
        loop {
            if let Some((_, deadline)) = dropped
                && Instant::now() >= deadline
            {
                return Ok(());
            }
            // SAFETY: display is live; QUEUED_AFTER_FLUSH reads the socket without blocking.
            if unsafe { XEventsQueued(display, QUEUED_AFTER_FLUSH) } == 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            let mut raw = MaybeUninit::<XEvent>::uninit();
            // SAFETY: events are queued, so XNextEvent returns without blocking.
            let event = unsafe {
                XNextEvent(display, raw.as_mut_ptr());
                raw.assume_init()
            };
            // SAFETY: type_ is the discriminator common to every XEvent arm.
            match unsafe { event.type_ } {
                MOTION_NOTIFY => {
                    // SAFETY: the discriminator selects XMotionEvent.
                    let motion = unsafe { event.xmotion };
                    time = motion.time;
                    if dropped.is_some() {
                        continue;
                    }
                    // SAFETY: display is live.
                    let found =
                        unsafe { self.xdnd_target_at(motion.x_root, motion.y_root, atoms.aware) };
                    if found.map(|f| f.0) != target.map(|t| t.0) {
                        if let Some((old, _)) = target {
                            unsafe { send_client(display, old, atoms.leave, window, [0; 4]) };
                        }
                        target = found.map(|(w, _)| (w, false));
                        if let Some((w, aware)) = found {
                            let version = protocol::source_version_for(aware);
                            unsafe {
                                send_client(
                                    display,
                                    w,
                                    atoms.enter,
                                    window,
                                    [(version << 24) as c_long, uri, plain, 0],
                                )
                            };
                        }
                        last_position = None;
                    }
                    if let Some((w, _)) = target
                        && last_position != Some((motion.x_root, motion.y_root))
                    {
                        let packed =
                            protocol::pack_position(motion.x_root, motion.y_root) as c_long;
                        unsafe {
                            send_client(
                                display,
                                w,
                                atoms.position,
                                window,
                                [0, packed, time as c_long, atoms.action_copy as c_long],
                            )
                        };
                        last_position = Some((motion.x_root, motion.y_root));
                    }
                }
                BUTTON_RELEASE => {
                    // SAFETY: the discriminator selects XButtonEvent.
                    time = unsafe { event.xbutton.time };
                    if dropped.is_some() {
                        continue;
                    }
                    match target {
                        Some((w, true)) => {
                            unsafe {
                                send_client(
                                    display,
                                    w,
                                    atoms.drop_,
                                    window,
                                    [0, time as c_long, 0, 0],
                                )
                            };
                            dropped = Some((w, Instant::now() + FINISH_TIMEOUT));
                        }
                        Some((w, false)) => {
                            unsafe { send_client(display, w, atoms.leave, window, [0; 4]) };
                            return Ok(());
                        }
                        None => return Ok(()),
                    }
                }
                KEY_PRESS => {
                    // SAFETY: the discriminator selects XKeyEvent.
                    let mut key = unsafe { event.xkey };
                    if unsafe { XLookupKeysym(&mut key, 0) } == XK_ESCAPE {
                        if let Some((w, _)) = target {
                            unsafe { send_client(display, w, atoms.leave, window, [0; 4]) };
                        }
                        return Ok(());
                    }
                    deferred.push(event);
                }
                CLIENT_MESSAGE => {
                    // SAFETY: the discriminator selects XClientMessageEvent.
                    let client = unsafe { event.xclient };
                    // SAFETY: format-32 client messages carry longs in `l`.
                    let from = unsafe { client.data.l[0] } as Window;
                    if client.message_type == atoms.status
                        && target.map(|t| t.0) == Some(from)
                        && let Some(state) = target.as_mut()
                    {
                        // SAFETY: format-32 client message; `l` is the live arm.
                        state.1 =
                            unsafe { client.data.l[1] } & (protocol::STATUS_ACCEPT as c_long) != 0;
                    } else if client.message_type == atoms.finished
                        && dropped.map(|d| d.0) == Some(from)
                    {
                        return Ok(());
                    }
                }
                SELECTION_REQUEST => {
                    // SAFETY: SelectionRequest shares SelectionRequestEvent's layout.
                    let request = unsafe {
                        ptr::read(&event as *const XEvent as *const SelectionRequestEvent)
                    };
                    if request.selection == atoms.selection {
                        // SAFETY: requestor is a live window named by the server.
                        unsafe { answer_selection_request(display, atoms, paths, &request) };
                    }
                }
                SELECTION_CLEAR => return Ok(()),
                _ => deferred.push(event),
            }
        }
    }
}

/// Serves `XdndSelection` as `TARGETS` or `text/uri-list`, refusing anything else.
///
/// # Safety
///
/// `display` must be live and `request.requestor` a valid window id.
unsafe fn answer_selection_request(
    display: *mut Display,
    atoms: XdndAtoms,
    paths: &[PathBuf],
    request: &SelectionRequestEvent,
) {
    let property = if request.property == 0 {
        request.target
    } else {
        request.property
    };
    let mut served = 0;
    // SAFETY: data buffers outlive the XChangeProperty calls that reference them.
    unsafe {
        if request.target == atoms.targets {
            let list: [c_long; 2] = [atoms.targets as c_long, atoms.uri_list as c_long];
            XChangeProperty(
                display,
                request.requestor,
                property,
                XA_ATOM,
                32,
                PROP_MODE_REPLACE,
                list.as_ptr().cast(),
                2,
            );
            served = property;
        } else if request.target == atoms.uri_list {
            let bytes = protocol::encode_uri_list(paths);
            if bytes.len() <= MAX_SERVED_BYTES {
                XChangeProperty(
                    display,
                    request.requestor,
                    property,
                    atoms.uri_list,
                    8,
                    PROP_MODE_REPLACE,
                    bytes.as_ptr(),
                    bytes.len() as c_int,
                );
                served = property;
            }
        }
        send_selection_notify(
            display,
            request.requestor,
            request.selection,
            request.target,
            served,
            request.time,
        );
    }
}
