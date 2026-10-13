//! Checked X11 pixmap gateway + process ledger (J01).
//!
//! All native pixmap traffic (backbuffer, image cache, masks) goes through
//! [`X11ResourceAllocator`]: dimension validation first, then a checked
//! `XCreatePixmap` under an error-trap lock + `XSync`, then ledger + gauge
//! accounting. Pure validation/ledger math is unit-testable without X.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::xlib::{
    Bool, Display, Drawable, Pixmap, XCreatePixmap, XErrorEvent, XFreePixmap, XSetErrorHandler,
    XSync,
};

/// Process-wide ledger snapshot for native pixmap resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceLedger {
    pub current_bytes: usize,
    pub peak_bytes: usize,
    pub live_count: usize,
    pub peak_count: usize,
    pub allocs: u64,
    pub frees: u64,
    pub failures: u64,
}

/// Byte size of one `w`x`h` pixmap at `depth` bits: `w*h*ceil(depth/8)`.
/// Saturates (never wraps); `usize::MAX` signals overflow to callers.
pub fn pixmap_byte_estimate(width: u32, height: u32, depth: u32) -> usize {
    let row = ((depth as usize).saturating_add(7)) / 8;
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(row))
        .unwrap_or(usize::MAX)
}

/// Pure dimension gate: `w,h > 0`, within the root screen, bytes checked.
/// Returns the native byte cost on success; never touches X.
pub fn validate_pixmap_dims(
    width: u32,
    height: u32,
    depth: u32,
    root_w: u32,
    root_h: u32,
) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("pixmap dimensions must be nonzero".to_string());
    }
    if width > root_w.max(1) || height > root_h.max(1) {
        return Err(format!(
            "pixmap {width}x{height} exceeds root {root_w}x{root_h}"
        ));
    }
    let bytes = pixmap_byte_estimate(width, height, depth);
    if bytes == usize::MAX {
        return Err(format!("pixmap {width}x{height}x{depth} size overflow"));
    }
    Ok(bytes)
}

fn set_gauge(label: &'static str, bytes: u64) {
    flamewm_profiler::MemoryGauge::new(label).set(bytes);
}

pub const PIXMAP_GAUGE: &str = "render.x11.pixmap_bytes";
pub const BACKBUFFER_GAUGE: &str = "render.x11.backbuffer_bytes";
pub const IMAGE_GAUGE: &str = "render.x11.image_bytes";

pub fn refresh_backbuffer_gauge(bytes: u64) {
    set_gauge(BACKBUFFER_GAUGE, bytes);
}

pub fn refresh_image_gauge(bytes: u64) {
    set_gauge(IMAGE_GAUGE, bytes);
}

static TRAP_LOCK: Mutex<()> = Mutex::new(());
static TRAP_FIRED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn trap_handler(
    _display: *mut Display,
    _event: *mut XErrorEvent,
) -> std::os::raw::c_int {
    TRAP_FIRED.store(true, Ordering::SeqCst);
    0
}

/// Checked pixmap owner: validates, traps X errors synchronously, and keeps
/// the process ledger (current/peak bytes + count, allocs/frees/failures).
#[derive(Debug, Default)]
pub struct X11ResourceAllocator {
    ledger: ResourceLedger,
}

impl X11ResourceAllocator {
    pub fn new() -> Self {
        Self {
            ledger: ResourceLedger::default(),
        }
    }

    #[cfg(test)]
    pub fn ledger(&self) -> ResourceLedger {
        self.ledger
    }

    fn refresh_pixmap_gauge(&self) {
        set_gauge(PIXMAP_GAUGE, self.ledger.current_bytes as u64);
    }

    fn note_alloc(&mut self, bytes: usize) {
        self.ledger.current_bytes = self.ledger.current_bytes.saturating_add(bytes);
        self.ledger.peak_bytes = self.ledger.peak_bytes.max(self.ledger.current_bytes);
        self.ledger.live_count = self.ledger.live_count.saturating_add(1);
        self.ledger.peak_count = self.ledger.peak_count.max(self.ledger.live_count);
        self.ledger.allocs = self.ledger.allocs.saturating_add(1);
        self.refresh_pixmap_gauge();
    }

    fn note_free(&mut self, bytes: usize) {
        self.ledger.current_bytes = self.ledger.current_bytes.saturating_sub(bytes);
        self.ledger.live_count = self.ledger.live_count.saturating_sub(1);
        self.ledger.frees = self.ledger.frees.saturating_add(1);
        self.refresh_pixmap_gauge();
    }

    fn note_failure(&mut self) {
        self.ledger.failures = self.ledger.failures.saturating_add(1);
    }

    /// Test/headless ledger hooks: record an accounted alloc/free without X.
    #[cfg(test)]
    pub fn note_alloc_for_test(&mut self, bytes: usize) {
        self.note_alloc(bytes);
    }

    /// Test/headless ledger hooks: record an accounted free without X.
    #[cfg(test)]
    pub fn note_free_for_test(&mut self, bytes: usize) {
        self.note_free(bytes);
    }

    /// Checked create: validate dims, then `XCreatePixmap` under the
    /// error-trap lock + `XSync`. Trap fire or zero pixmap is an accounted
    /// failure; success records the byte cost in the ledger.
    ///
    /// # Safety
    /// `display`/`drawable` must be live on the calling thread.
    pub unsafe fn create_pixmap(
        &mut self,
        display: *mut Display,
        drawable: Drawable,
        width: u32,
        height: u32,
        depth: u32,
        root_w: u32,
        root_h: u32,
    ) -> Result<(Pixmap, usize), String> {
        let bytes = validate_pixmap_dims(width, height, depth, root_w, root_h)?;
        let _trap = TRAP_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        TRAP_FIRED.store(false, Ordering::SeqCst);
        let previous = unsafe { XSetErrorHandler(Some(trap_handler)) };
        // Drain earlier queued requests first: their async errors would otherwise be
        // reported by this call's XSync and misattributed to this pixmap.
        unsafe { XSync(display, 0 as Bool) };
        TRAP_FIRED.store(false, Ordering::SeqCst);
        // SAFETY: display/drawable are live per the caller contract.
        let pixmap = unsafe { XCreatePixmap(display, drawable, width, height, depth) };
        unsafe { XSync(display, 0 as Bool) };
        unsafe { XSetErrorHandler(previous) };
        if TRAP_FIRED.load(Ordering::SeqCst) || pixmap == 0 {
            self.note_failure();
            return Err(format!(
                "XCreatePixmap failed for {width}x{height}x{depth} pixmap"
            ));
        }
        self.note_alloc(bytes);
        Ok((pixmap, bytes))
    }

    /// Checked free: `XFreePixmap`, then subtract the tracked byte cost.
    ///
    /// # Safety
    /// `display`/`pixmap` must be live on the calling thread.
    pub unsafe fn free_pixmap(&mut self, display: *mut Display, pixmap: Pixmap, bytes: usize) {
        if pixmap == 0 {
            return;
        }
        // SAFETY: display/pixmap are live per the caller contract.
        unsafe { XFreePixmap(display, pixmap) };
        self.note_free(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_and_oversize_dims() {
        assert!(validate_pixmap_dims(0, 8, 32, 800, 600).is_err());
        assert!(validate_pixmap_dims(8, 0, 32, 800, 600).is_err());
        assert!(validate_pixmap_dims(801, 8, 32, 800, 600).is_err());
        assert!(validate_pixmap_dims(8, 601, 32, 800, 600).is_err());
        assert_eq!(validate_pixmap_dims(2, 3, 32, 800, 600).unwrap(), 24);
        assert_eq!(validate_pixmap_dims(2, 3, 1, 800, 600).unwrap(), 6);
    }

    #[test]
    fn checked_bytes_never_wrap() {
        assert_eq!(pixmap_byte_estimate(u32::MAX, u32::MAX, 32), usize::MAX);
        assert!(validate_pixmap_dims(u32::MAX, u32::MAX, 32, u32::MAX, u32::MAX).is_err());
    }

    /// A failed request on an earlier, unsynced call (here a `XDeleteProperty` on a
    /// destroyed window) must not be attributed to a later, valid `XCreatePixmap`.
    /// Needs a live X server (`FLAMEWM_TEST_DISPLAY`); skipped otherwise.
    #[test]
    fn stale_async_error_is_not_attributed_to_later_pixmap() {
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        let display = unsafe { super::super::super::xlib::XOpenDisplay(name.as_ptr()) };
        assert!(!display.is_null(), "XOpenDisplay failed");
        unsafe {
            let screen = super::super::super::xlib::XDefaultScreen(display);
            let root = super::super::super::xlib::XRootWindow(display, screen);
            let window =
                super::super::super::xlib::XCreateSimpleWindow(display, root, 0, 0, 8, 8, 0, 0, 0);
            super::super::super::xlib::XSync(display, 0);
            super::super::super::xlib::XDestroyWindow(display, window);
            let atom_name = std::ffi::CString::new("FLAMEWM_STALE_ERROR_TEST").unwrap();
            let atom = super::super::super::xlib::XInternAtom(display, atom_name.as_ptr(), 0);
            // Async BadWindow: queued on a destroyed window, not yet synced.
            super::super::super::xlib::XDeleteProperty(display, window, atom);
            let mut alloc = X11ResourceAllocator::new();
            let (root_w, root_h) = (1280, 800);
            let result = alloc.create_pixmap(display, root, 25, 25, 32, root_w, root_h);
            super::super::super::xlib::XCloseDisplay(display);
            assert!(result.is_ok(), "stale error misattributed: {result:?}");
            assert_eq!(alloc.ledger().failures, 0);
        }
    }

    #[test]
    fn ledger_tracks_current_peak_and_failures() {
        let mut alloc = X11ResourceAllocator::new();
        alloc.note_alloc_for_test(100);
        alloc.note_alloc_for_test(50);
        let snap = alloc.ledger();
        assert_eq!(
            snap,
            ResourceLedger {
                current_bytes: 150,
                peak_bytes: 150,
                live_count: 2,
                peak_count: 2,
                allocs: 2,
                frees: 0,
                failures: 0,
            }
        );
        alloc.note_free_for_test(100);
        let snap = alloc.ledger();
        assert_eq!(snap.current_bytes, 50);
        assert_eq!(snap.peak_bytes, 150);
        assert_eq!(snap.live_count, 1);
        assert_eq!((snap.allocs, snap.frees), (2, 1));
        alloc.note_failure();
        assert_eq!(alloc.ledger().failures, 1);
    }
}
