use std::collections::HashSet;
use std::ffi::CStr;
use std::os::raw::c_char;

use super::*;

// Input-focus FFI is not in the pinned xlib.rs surface. It is declared here
// beside its only callers, the popup grab lifecycle that also owns XGrabPointer.
unsafe extern "C" {
    pub(crate) fn XGetInputFocus(
        display: *mut Display,
        focus_return: *mut Window,
        revert_to_return: *mut std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    pub(crate) fn XSetInputFocus(
        display: *mut Display,
        focus: Window,
        revert_to: std::os::raw::c_int,
        time: Time,
    ) -> std::os::raw::c_int;
    /// Reads WM_NAME of `window`; the caller frees the string with XFree.
    pub(crate) fn XFetchName(
        display: *mut Display,
        window: Window,
        window_name_return: *mut *mut c_char,
    ) -> std::os::raw::c_int;
}

/// Selection key of the quick-popup press observer. Never a real surface id.
pub(crate) const PRESS_OBSERVER: SurfaceId = SurfaceId(u64::MAX);

/// One root press seen while the press observer is armed. `child_window_title`
/// is the WM_NAME of the top-level window under the pointer, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPressObservation {
    pub x_root: i32,
    pub y_root: i32,
    pub button: u32,
    pub time_ms: u64,
    pub child_window_title: Option<String>,
}

pub(super) const REVERT_TO_POINTER_ROOT: std::os::raw::c_int = 1;

/// XInput2 2.0 raw selection for popup outside-dismissal. RawButtonPress is
/// delivered to this client without any pointer grab, so every click reaches the
/// window beneath it. Selected only while a popup holds its pointer grab.
/// The root selection is shared by every armed popup: it is written when the
/// first surface arms and cleared only when the last armed surface disarms.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RawPressSelection {
    opcode: Option<std::os::raw::c_int>,
    armed: HashSet<SurfaceId>,
}

impl RawPressSelection {
    pub(crate) fn new(opcode: Option<std::os::raw::c_int>) -> Self {
        Self { opcode, armed: HashSet::new() }
    }

    /// XInput2 major opcode of the raw events this selection accepts.
    pub(crate) fn opcode(&self) -> Option<std::os::raw::c_int> {
        self.opcode
    }

    /// Arms `surface`. Returns true only when the set goes empty -> non-empty,
    /// i.e. the root selection must be written. Idempotent per surface.
    pub(crate) fn arm(&mut self, surface: SurfaceId) -> bool {
        if self.opcode.is_none() {
            return false;
        }
        let was_empty = self.armed.is_empty();
        self.armed.insert(surface) && was_empty
    }

    /// Disarms `surface`. Returns true only when the set goes non-empty -> empty,
    /// i.e. the root selection must be cleared. Idempotent per surface.
    pub(crate) fn disarm(&mut self, surface: SurfaceId) -> bool {
        self.armed.remove(&surface) && self.armed.is_empty()
    }
}

/// Event mask bits for XI_RawButtonPress only.
pub(super) fn raw_press_mask() -> [u8; 2] {
    let mut mask = [0u8; 2];
    mask[(XI_RAW_BUTTON_PRESS >> 3) as usize] |= 1 << (XI_RAW_BUTTON_PRESS & 7);
    mask
}

/// Select (`armed`) or clear this client's RawButtonPress selection on the root.
/// An empty mask (mask_len 0) removes only this client's selection.
pub(super) unsafe fn write_raw_press_selection(display: *mut Display, root: Window, armed: bool) {
    let mut mask = raw_press_mask();
    let mut event_mask = XIEventMask {
        deviceid: XI_ALL_MASTER_DEVICES,
        mask_len: if armed { mask.len() as std::os::raw::c_int } else { 0 },
        mask: mask.as_mut_ptr(),
    };
    // SAFETY: live connection; the mask outlives the request.
    unsafe {
        XISelectEvents(display, root, &mut event_mask, 1);
        XFlush(display);
    }
}

/// WM_NAME of `window`, or `None` when unset or unreadable.
pub(super) unsafe fn window_title(display: *mut Display, window: Window) -> Option<String> {
    let mut name: *mut c_char = std::ptr::null_mut();
    // SAFETY: live connection; `name` is freed below when XFetchName returns one.
    unsafe {
        if XFetchName(display, window, &mut name) == 0 || name.is_null() {
            return None;
        }
        let title = CStr::from_ptr(name).to_string_lossy().into_owned();
        XFree(name.cast());
        Some(title)
    }
}

/// XInput2 opcode when the server speaks XI 2.0. `None` disables raw dismissal;
/// FocusOut and Escape still dismiss the popup.
pub(crate) unsafe fn xi_raw_press_opcode(display: *mut Display) -> Option<std::os::raw::c_int> {
    // SAFETY: live connection; out-pointers are locals.
    unsafe {
        let mut opcode: std::os::raw::c_int = 0;
        let mut first_event: std::os::raw::c_int = 0;
        let mut first_error: std::os::raw::c_int = 0;
        if XQueryExtension(
            display,
            b"XInputExtension\0".as_ptr().cast(),
            &mut opcode,
            &mut first_event,
            &mut first_error,
        ) == 0
        {
            return None;
        }
        let mut major: std::os::raw::c_int = 2;
        let mut minor: std::os::raw::c_int = 0;
        let status = XIQueryVersion(display, &mut major, &mut minor);
        (status == 0 && (major, minor) >= (2, 0)).then_some(opcode)
    }
}

pub(crate) unsafe fn grab_popup_presses(
    display: *mut Display,
    root: Window,
    selection: &mut RawPressSelection,
    surface: SurfaceId,
) {
    if selection.arm(surface) {
        // SAFETY: display/root are the live controller connection and root window.
        unsafe { write_raw_press_selection(display, root, true) };
    }
}

pub(crate) unsafe fn release_popup_presses(
    display: *mut Display,
    root: Window,
    selection: &mut RawPressSelection,
    surface: SurfaceId,
) {
    if selection.disarm(surface) {
        // SAFETY: same live connection; the selection was armed by grab_popup_presses.
        unsafe { write_raw_press_selection(display, root, false) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabbedPressOutcome {
    Inside,
    Outside,
}

/// Classify a root-space press against the popup rect (device px).
pub fn classify_grabbed_press(
    x_root: i32,
    y_root: i32,
    origin: (i32, i32),
    width: u32,
    height: u32,
) -> GrabbedPressOutcome {
    if button_release_is_outside(x_root - origin.0, y_root - origin.1, width, height) {
        GrabbedPressOutcome::Outside
    } else {
        GrabbedPressOutcome::Inside
    }
}

/// Pure mapping of one root RawButtonPress to the popup's outside-dismiss event.
/// `popup` is (origin, size) in device px; `None` for any other event type or a
/// press inside the popup rect.
pub(crate) fn raw_press_outside_event(
    surface: SurfaceId,
    event_type: std::os::raw::c_int,
    button: u32,
    press: (i32, i32),
    popup: ((i32, i32), (u32, u32)),
    scale: f32,
    time_ms: u64,
) -> Option<SurfaceControllerEvent> {
    let (origin, (width, height)) = popup;
    if event_type != XI_RAW_BUTTON_PRESS
        || classify_grabbed_press(press.0, press.1, origin, width, height) != GrabbedPressOutcome::Outside
    {
        return None;
    }
    Some(outside_release_event(
        surface,
        button,
        (press.0 - origin.0) as f32 / scale,
        (press.1 - origin.1) as f32 / scale,
        time_ms,
    ))
}

/// Popup focus handoff, called where the popup takes its pointer grab. Focus
/// moves to the popup so Escape reaches it from any pointer position, and the
/// displaced focus is kept on the app. Passive WM key grabs (Super) keep firing
/// because the popup sits under the root; no active keyboard grab is taken.
/// X errors (e.g. BadMatch) are non-fatal: the process error handler ignores
/// protocol errors.
pub(crate) unsafe fn popup_take_focus(display: *mut Display, app: &mut X11App) {
    if x_io_broken() {
        return;
    }
    // SAFETY: display is the live controller connection; out-pointers are locals.
    unsafe {
        if app.saved_input_focus.is_none() {
            let mut focus: Window = 0;
            let mut revert: std::os::raw::c_int = 0;
            XGetInputFocus(display, &mut focus, &mut revert);
            app.saved_input_focus = Some((focus, revert));
        }
        XSetInputFocus(display, app.window, REVERT_TO_POINTER_ROOT, CURRENT_TIME);
    }
}

/// Restore the focus displaced by `popup_take_focus`, only while the popup
/// still holds focus, so WM focus changes made during the popup are kept. A
/// stale saved window fails with BadWindow (ignored); the X server then leaves
/// focus on PointerRoot once the popup is unmapped.
pub(crate) unsafe fn popup_restore_focus(display: *mut Display, app: &mut X11App) {
    let Some((previous, revert)) = app.saved_input_focus.take() else {
        return;
    };
    if x_io_broken() {
        return;
    }
    // SAFETY: display is the live controller connection; out-pointers are locals.
    unsafe {
        let mut current: Window = 0;
        let mut current_revert: std::os::raw::c_int = 0;
        XGetInputFocus(display, &mut current, &mut current_revert);
        if current == app.window {
            XSetInputFocus(display, previous, revert, CURRENT_TIME);
        }
    }
}

/// Outside presses during a slide-out are ignored: the popup is already closing and must not
/// dismiss again while its grab is still held.
pub(super) fn slide_out_swallows_press(active: Option<SlideKind>) -> bool {
    active.is_some_and(|kind| !kind.opening)
}

impl SurfaceController {
    pub(super) fn root_window(&self) -> Window {
        // SAFETY: the controller owns the live display; root is a plain query.
        unsafe { XRootWindow(self.display, self.screen) }
    }

    /// Dismiss the grabbed popup when a root RawButtonPress lands outside it. The
    /// press is only observed: it is never grabbed, replayed or consumed.
    pub(super) fn dismiss_on_raw_press(
        &mut self,
        event: &XEvent,
        on_action: &mut impl FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    ) -> Result<(), String> {
        let display = self.display;
        // SAFETY: `event` is a GenericEvent per the caller's `GENERIC_EVENT` check.
        let mut cookie = unsafe { generic_cookie(event) };
        // SAFETY: the cookie is a copy of this XNextEvent generic event; Xlib fills `data`.
        if unsafe { XGetEventData(display, &mut cookie) } == 0 {
            return Ok(());
        }
        // SAFETY: XGetEventData succeeded, so `data` is an XIRawEvent until XFreeEventData.
        let raw = unsafe { *(cookie.data as *const XIRawEvent) };
        unsafe { XFreeEventData(display, &mut cookie) };
        if raw.evtype != XI_RAW_BUTTON_PRESS {
            return Ok(());
        }
        let root = self.root_window();
        let mut root_return: Window = 0;
        let mut child_return: Window = 0;
        let (mut x_root, mut y_root, mut win_x, mut win_y) = (0, 0, 0, 0);
        let mut mask: std::os::raw::c_uint = 0;
        // SAFETY: live connection; out-pointers are locals.
        let on_screen = unsafe {
            XQueryPointer(
                display,
                root,
                &mut root_return,
                &mut child_return,
                &mut x_root,
                &mut y_root,
                &mut win_x,
                &mut win_y,
                &mut mask,
            )
        };
        if on_screen == 0 {
            return Ok(());
        }
        if self.press_observer_armed {
            let title = (child_return != 0)
                .then(|| unsafe { window_title(display, child_return) })
                .flatten();
            self.raw_press_observations.push(RawPressObservation {
                x_root,
                y_root,
                button: raw.detail as u32,
                time_ms: raw.time as u64,
                child_window_title: title,
            });
        }
        let owner = self
            .surfaces
            .iter()
            .find(|(id, instance)| {
                instance.app.pointer_grabbed && !slide_out_swallows_press(self.slides.kind(id.get()))
            })
            .map(|(id, instance)| {
                (*id, instance.origin, (instance.app.width, instance.app.height), instance.document.ui_scale())
            });
        let Some((id, origin, size, scale)) = owner else {
            return Ok(());
        };
        let Some(outside) = raw_press_outside_event(
            id,
            raw.evtype,
            raw.detail as u32,
            (x_root, y_root),
            (origin, size),
            scale,
            raw.time as u64,
        ) else {
            return Ok(());
        };
        let instance = self.instance_mut(id)?;
        on_action(&outside, &mut instance.document)
    }

    /// Observe root presses (no grab, no replay) while quick popups are open.
    /// Idempotent; the observer shares the root selection with popup grabs.
    pub fn arm_raw_press_observer(&mut self) {
        if self.press_observer_armed {
            return;
        }
        self.press_observer_armed = true;
        // SAFETY: live controller connection and root window.
        unsafe {
            grab_popup_presses(self.display, self.root_window(), &mut self.press_selection, PRESS_OBSERVER)
        };
    }

    /// Stop observing and drop anything still queued. Idempotent.
    pub fn disarm_raw_press_observer(&mut self) {
        if !self.press_observer_armed {
            return;
        }
        self.press_observer_armed = false;
        self.raw_press_observations.clear();
        // SAFETY: same live connection; armed by arm_raw_press_observer.
        unsafe {
            release_popup_presses(self.display, self.root_window(), &mut self.press_selection, PRESS_OBSERVER)
        };
    }

    /// Root presses observed since the last call, in arrival order.
    pub fn take_raw_press_observations(&mut self) -> Vec<RawPressObservation> {
        std::mem::take(&mut self.raw_press_observations)
    }

}
