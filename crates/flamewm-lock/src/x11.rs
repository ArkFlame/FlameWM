//! Xlib keyboard and pointer grabs on the root window, plus key decoding.
//! Grabs live on the lock screen's own connection, so every key press reaches
//! this process and never the WM's passive shortcut grabs.
use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong};
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
const MOD4_MASK: c_uint = 1 << 6;

const KEYSYM_BACKSPACE: c_ulong = 0xff08;
const KEYSYM_TAB: c_ulong = 0xff09;
const KEYSYM_LINEFEED: c_ulong = 0xff0a;
const KEYSYM_RETURN: c_ulong = 0xff0d;
const KEYSYM_ESCAPE: c_ulong = 0xff1b;
const KEYSYM_KP_ENTER: c_ulong = 0xff8d;
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
}

/// An owned connection that holds the keyboard and pointer grabs while alive.
pub struct Session {
    display: *mut Display,
    root: c_ulong,
    grabbed: bool,
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
        })
    }

    /// Descriptor readable when key events are pending.
    pub fn fd(&self) -> RawFd {
        // SAFETY: `display` is live for the lifetime of `self`.
        unsafe { XConnectionNumber(self.display) }
    }

    /// Takes the keyboard and pointer grabs on the root window. A keyboard
    /// grab taken without the pointer is released again. Idempotent.
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
        if keyboard != GRAB_SUCCESS {
            self.grabbed = false;
            return false;
        }
        // SAFETY: as above; an empty event mask reports no pointer events.
        let pointer = unsafe {
            XGrabPointer(
                self.display,
                self.root,
                0,
                0,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                0,
                0,
                CURRENT_TIME,
            )
        };
        if pointer != GRAB_SUCCESS {
            // SAFETY: display is live.
            unsafe {
                XUngrabKeyboard(self.display, CURRENT_TIME);
                XFlush(self.display);
            }
            self.grabbed = false;
            return false;
        }
        // SAFETY: display is live.
        unsafe { XFlush(self.display) };
        self.grabbed = true;
        true
    }

    /// Drains pending events and returns the key presses among them.
    pub fn read_keys(&mut self) -> Vec<Key> {
        let mut keys = Vec::new();
        // SAFETY: display is live; XNextEvent only runs while XPending reports events.
        unsafe {
            while XPending(self.display) > 0 {
                let mut event = XEventBuffer([0; 24]);
                XNextEvent(self.display, &mut event);
                if event.0[0] as c_int == KEY_PRESS {
                    let key = &*(event.0.as_ptr().cast::<XKeyEvent>());
                    keys.extend(decode_key(self.display, key));
                }
            }
        }
        keys
    }

    fn release(&mut self) {
        if self.grabbed {
            // SAFETY: display is live until XCloseDisplay in Drop.
            unsafe {
                XUngrabPointer(self.display, CURRENT_TIME);
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
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(keysym as u32).map(Key::Char),
        KEYSYM_UNICODE_BASE..=0x0110_ffff => {
            char::from_u32((keysym - KEYSYM_UNICODE_BASE) as u32).map(Key::Char)
        }
        _ => None,
    }
}
