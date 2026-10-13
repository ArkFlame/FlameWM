//! Xlib keyboard and pointer grabs on the root window, plus key decoding.
//! Grabs live on the lock screen's own connection, so every key press reaches
//! this process and never the WM's passive shortcut grabs.
use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};
use std::os::fd::RawFd;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use crate::state::Key;

const CURRENT_TIME: c_ulong = 0;
const GRAB_MODE_ASYNC: c_int = 1;
const GRAB_SUCCESS: c_int = 0;
const KEY_PRESS: c_int = 2;
const SHIFT_MASK: c_uint = 1 << 0;
const CONTROL_MASK: c_uint = 1 << 2;
const MOD1_MASK: c_uint = 1 << 3;
const LOCK_MASK: c_uint = 1 << 1;
const MOD4_MASK: c_uint = 1 << 6;
const BUTTON_MASK: c_uint = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);
const DPMS_MODE_ON: u16 = 0;
const DPMS_MODE_OFF: u16 = 3;
const RTLD_NOW: c_int = 2;

const KEYSYM_BACKSPACE: c_ulong = 0xff08;
const KEYSYM_TAB: c_ulong = 0xff09;
const KEYSYM_LINEFEED: c_ulong = 0xff0a;
const KEYSYM_RETURN: c_ulong = 0xff0d;
const KEYSYM_ESCAPE: c_ulong = 0xff1b;
const KEYSYM_KP_ENTER: c_ulong = 0xff8d;
const KEYSYM_F1: c_ulong = 0xffbe;
const KEYSYM_F4: c_ulong = 0xffc1;
const KEYSYM_UNICODE_BASE: c_ulong = 0x0100_0000;

/// Interval between grab attempts while waiting for another client to let go.
const GRAB_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Opaque Xlib `Display`.
#[repr(C)]
pub struct Display {
    _private: [u8; 0],
}

/// Layout of Xlib's `XKeyEvent` on LP64 Linux.
#[repr(C)]
struct XKeyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: c_ulong,
    root: c_ulong,
    subwindow: c_ulong,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
    same_screen: c_int,
}

/// Raw storage for Xlib's `XEvent` union (192 bytes on LP64).
#[repr(C)]
struct XEventBuffer([c_long; 24]);

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XDefaultRootWindow(display: *mut Display) -> c_ulong;
    fn XConnectionNumber(display: *mut Display) -> c_int;
    fn XPending(display: *mut Display) -> c_int;
    fn XNextEvent(display: *mut Display, event: *mut XEventBuffer) -> c_int;
    fn XKeycodeToKeysym(display: *mut Display, keycode: c_uchar, index: c_int) -> c_ulong;
    fn XGrabKeyboard(
        display: *mut Display,
        grab_window: c_ulong,
        owner_events: c_int,
        pointer_mode: c_int,
        keyboard_mode: c_int,
        time: c_ulong,
    ) -> c_int;
    fn XUngrabKeyboard(display: *mut Display, time: c_ulong) -> c_int;
    fn XGrabPointer(
        display: *mut Display,
        grab_window: c_ulong,
        owner_events: c_int,
        event_mask: c_uint,
        pointer_mode: c_int,
        keyboard_mode: c_int,
        confine_to: c_ulong,
        cursor: c_ulong,
        time: c_ulong,
    ) -> c_int;
    fn XUngrabPointer(display: *mut Display, time: c_ulong) -> c_int;
    fn XFlush(display: *mut Display) -> c_int;
    fn XQueryKeymap(display: *mut Display, keys_return: *mut c_char) -> c_int;
    fn XQueryPointer(
        display: *mut Display,
        window: c_ulong,
        root_return: *mut c_ulong,
        child_return: *mut c_ulong,
        root_x: *mut c_int,
        root_y: *mut c_int,
        win_x: *mut c_int,
        win_y: *mut c_int,
        mask_return: *mut c_uint,
    ) -> c_int;
}

// The DPMS extension lives in libXext, which is opened at run time, never linked.
// dlopen/dlsym come from the C library that std already links.
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type DpmsEnableFn = unsafe extern "C" fn(*mut Display) -> c_int;
type DpmsForceLevelFn = unsafe extern "C" fn(*mut Display, u16) -> c_int;

/// DPMS entry points resolved from libXext at run time.
#[derive(Clone, Copy)]
struct Dpms {
    enable: DpmsEnableFn,
    force_level: DpmsForceLevelFn,
}

impl Dpms {
    /// Resolves the DPMS calls, or `None` when libXext or the symbols are absent.
    fn load() -> Option<Self> {
        // SAFETY: plain C library lookups with NUL-terminated names; the handle is never closed.
        unsafe {
            let handle = dlopen(c"libXext.so.6".as_ptr(), RTLD_NOW);
            if handle.is_null() {
                return None;
            }
            let enable = dlsym(handle, c"DPMSEnable".as_ptr());
            let force_level = dlsym(handle, c"DPMSForceLevel".as_ptr());
            if enable.is_null() || force_level.is_null() {
                return None;
            }
            Some(Self {
                // SAFETY: the symbols are the documented libXext signatures.
                enable: std::mem::transmute::<*mut c_void, DpmsEnableFn>(enable),
                force_level: std::mem::transmute::<*mut c_void, DpmsForceLevelFn>(force_level),
            })
        }
    }
}

/// The two DPMS calls the lock makes, behind a trait so the sequence is unit-tested.
pub trait DpmsCalls {
    /// `DPMSEnable`: the server must have DPMS enabled before a forced level takes effect.
    fn enable(&mut self) -> bool;
    /// `DPMSForceLevel` with a DPMS level (0 = on, 3 = off).
    fn force_level(&mut self, level: u16) -> bool;
}

/// Display off is enable-then-force-off; display on is force-on. Returns the force result.
pub fn set_power(calls: &mut impl DpmsCalls, on: bool) -> bool {
    if on {
        calls.force_level(DPMS_MODE_ON)
    } else {
        calls.enable();
        calls.force_level(DPMS_MODE_OFF)
    }
}

struct XDpms {
    display: *mut Display,
    dpms: Dpms,
}

impl DpmsCalls for XDpms {
    fn enable(&mut self) -> bool {
        // SAFETY: the display is live for the owning Session; the symbol came from libXext.
        unsafe { (self.dpms.enable)(self.display) != 0 }
    }

    fn force_level(&mut self, level: u16) -> bool {
        // SAFETY: as above.
        unsafe { (self.dpms.force_level)(self.display, level) != 0 }
    }
}

/// Pointer position and modifier/button state from `XQueryPointer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerState {
    pub x: i32,
    pub y: i32,
    pub mask: u32,
}

impl PointerState {
    /// Button or pointer activity relative to `previous`: a move or a button change.
    pub fn moved_or_buttons_changed(&self, previous: &Self) -> bool {
        self.x != previous.x || self.y != previous.y || (self.mask & BUTTON_MASK) != (previous.mask & BUTTON_MASK)
    }

    /// Caps Lock is on when the server reports the Lock modifier.
    pub fn caps_lock(&self) -> bool {
        self.mask & LOCK_MASK != 0
    }
}

/// Keyboard input drained in one call: decoded keys plus every raw key press.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyBatch {
    pub keys: Vec<Key>,
    /// Raw `KeyPress` events, including keys that decode to nothing (wake input).
    pub presses: usize,
}

/// An owned connection that holds the keyboard and pointer grabs while alive.
pub struct Session {
    display: *mut Display,
    root: c_ulong,
    grabbed: bool,
    dpms: Option<Dpms>,
}

impl Session {
    /// Opens the default display. Grabs are not taken yet.
    pub fn open() -> Result<Self, String> {
        // SAFETY: a null name selects $DISPLAY; the result is checked below.
        let display = unsafe { XOpenDisplay(ptr::null()) };
        if display.is_null() {
            return Err("cannot open X display".to_string());
        }
        // SAFETY: `display` is a live connection checked non-null above.
        let root = unsafe { XDefaultRootWindow(display) };
        Ok(Self {
            display,
            root,
            grabbed: false,
            dpms: Dpms::load(),
        })
    }

    /// Descriptor readable when key events are pending.
    pub fn fd(&self) -> RawFd {
        // SAFETY: `display` is live for the lifetime of `self`.
        unsafe { XConnectionNumber(self.display) }
    }

    /// Takes the keyboard grab on the root window. The pointer grab is taken on the
    /// lock surface itself (`SurfaceRuntime::grab_pointer`), so presses reach its
    /// controls. Idempotent; the lock is never dropped by a failed retry.
    pub fn try_grab(&mut self) -> bool {
        // SAFETY: `display` is live; root is a valid window of this display.
        let keyboard = unsafe {
            XGrabKeyboard(
                self.display,
                self.root,
                0,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                CURRENT_TIME,
            )
        };
        self.grabbed = keyboard == GRAB_SUCCESS;
        // SAFETY: display is live.
        unsafe { XFlush(self.display) };
        self.grabbed
    }

    /// Drains pending events: the decoded key presses and the count of all key presses.
    pub fn read_keys(&mut self) -> KeyBatch {
        let mut batch = KeyBatch::default();
        // SAFETY: display is live; XNextEvent only runs while XPending reports events.
        unsafe {
            while XPending(self.display) > 0 {
                let mut event = XEventBuffer([0; 24]);
                XNextEvent(self.display, &mut event);
                if event.0[0] as c_int == KEY_PRESS {
                    batch.presses += 1;
                    let key = &*(event.0.as_ptr().cast::<XKeyEvent>());
                    batch.keys.extend(decode_key(self.display, key));
                }
            }
        }
        batch
    }

    /// Current pointer position and modifier/button mask. Works without a pointer
    /// event mask, so it sees motion and buttons while the grab reports no events.
    pub fn pointer(&self) -> Option<PointerState> {
        let (mut root_return, mut child_return) = (0, 0);
        let (mut root_x, mut root_y, mut win_x, mut win_y) = (0, 0, 0, 0);
        let mut mask: c_uint = 0;
        // SAFETY: display is live; every out-pointer refers to a local of the right type.
        let same_screen = unsafe {
            XQueryPointer(
                self.display,
                self.root,
                &mut root_return,
                &mut child_return,
                &mut root_x,
                &mut root_y,
                &mut win_x,
                &mut win_y,
                &mut mask,
            )
        };
        (same_screen != 0).then_some(PointerState {
            x: root_x,
            y: root_y,
            mask,
        })
    }

    /// Bitmap of the keys held down, one bit per keycode, read without any grab.
    pub fn keymap(&self) -> [u8; 32] {
        let mut keys = [0 as c_char; 32];
        // SAFETY: display is live; XQueryKeymap writes exactly 32 bytes into the buffer.
        unsafe { XQueryKeymap(self.display, keys.as_mut_ptr()) };
        keys.map(|byte| byte as u8)
    }

    /// Turns the display off (`on == false`) or back on through DPMS. `false` when the
    /// extension is unavailable or the server refuses; the lock stays up either way.
    pub fn set_display_power(&mut self, on: bool) -> bool {
        let Some(dpms) = self.dpms else {
            return false;
        };
        let ok = set_power(
            &mut XDpms {
                display: self.display,
                dpms,
            },
            on,
        );
        // SAFETY: display is live.
        unsafe { XFlush(self.display) };
        ok
    }

    fn release(&mut self) {
        if self.grabbed {
            // SAFETY: display is live until XCloseDisplay in Drop.
            unsafe {
                XUngrabKeyboard(self.display, CURRENT_TIME);
                XFlush(self.display);
            }
            self.grabbed = false;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.release();
        // SAFETY: the connection was opened by `open` and is closed exactly once.
        unsafe {
            XCloseDisplay(self.display);
        }
    }
}

/// Retries [`Session::try_grab`] until it succeeds or `timeout` elapses.
pub fn grab_with_retry(session: &mut Session, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if session.try_grab() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(GRAB_RETRY_INTERVAL);
    }
}

/// Maps a key press to a lock key. Only unmodified or Shift-modified printable
/// keys produce characters; Control, Alt and Super combinations are dropped.
///
/// # Safety
/// `display` must be a live connection and `event` a `KeyPress` from it.
unsafe fn decode_key(display: *mut Display, event: &XKeyEvent) -> Option<Key> {
    if event.state & (CONTROL_MASK | MOD1_MASK | MOD4_MASK) != 0 {
        return None;
    }
    let index = c_int::from(event.state & SHIFT_MASK != 0);
    // SAFETY: the caller guarantees a live display; the keycode comes from the server.
    let keysym = unsafe { XKeycodeToKeysym(display, event.keycode as c_uchar, index) };
    match keysym {
        KEYSYM_RETURN | KEYSYM_KP_ENTER | KEYSYM_LINEFEED => Some(Key::Enter),
        KEYSYM_BACKSPACE => Some(Key::Backspace),
        KEYSYM_ESCAPE => Some(Key::Escape),
        KEYSYM_TAB => None,
        KEYSYM_F1..=KEYSYM_F4 => Some(Key::Function((keysym - KEYSYM_F1) as u8 + 1)),
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(keysym as u32).map(Key::Char),
        KEYSYM_UNICODE_BASE..=0x0110_ffff => {
            char::from_u32((keysym - KEYSYM_UNICODE_BASE) as u32).map(Key::Char)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        result: bool,
    }

    impl DpmsCalls for Recorder {
        fn enable(&mut self) -> bool {
            self.calls.push("enable".to_string());
            true
        }

        fn force_level(&mut self, level: u16) -> bool {
            self.calls.push(format!("force({level})"));
            self.result
        }
    }

    #[test]
    fn display_off_enables_dpms_then_forces_off() {
        let mut calls = Recorder { result: true, ..Recorder::default() };
        assert!(set_power(&mut calls, false));
        assert_eq!(calls.calls, vec!["enable", "force(3)"]);
    }

    #[test]
    fn display_on_forces_on_without_enable() {
        let mut calls = Recorder { result: true, ..Recorder::default() };
        assert!(set_power(&mut calls, true));
        assert_eq!(calls.calls, vec!["force(0)"]);
    }

    #[test]
    fn refused_force_is_reported_as_failure() {
        let mut calls = Recorder::default();
        assert!(!set_power(&mut calls, false));
        assert_eq!(calls.calls, vec!["enable", "force(3)"]);
    }
}
