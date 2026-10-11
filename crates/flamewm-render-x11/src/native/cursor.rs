//! Xcursor backend: semantic cursor names via libXcursor, cached,
//! with core font-cursor fallback only on Xcursor failure.
//! Logs `FLAMEWM_RENDER_CURSOR_BACKEND` exactly once per process.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

use flamewm_render_core::CursorKind;

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xft::XftBackend;
use crate::xlib::{
    Cursor, Display, Window, XCURSOR_LIBRARY_NAMES, XCURSOR_SET_SIZE_SYMBOL,
    XCURSOR_SET_THEME_SYMBOL,
};
use crate::xrender::XRenderBackend;
use crate::xshape::XShapeBridge;

type LibraryLoadCursorFn = unsafe extern "C" fn(*mut Display, *const c_char) -> Cursor;
type LibrarySetThemeFn = unsafe extern "C" fn(*mut Display, *const c_char);
type LibrarySetSizeFn = unsafe extern "C" fn(*mut Display, c_int);

static LOG_ONCE: Once = Once::new();

fn log_backend(backend: &str) {
    LOG_ONCE.call_once(|| {
        eprintln!("FLAMEWM_RENDER_CURSOR_BACKEND {backend}");
    });
}

pub struct XcursorBackend {
    display: *mut Display,
    load: Option<LibraryLoadCursorFn>,
    _library: Option<DynamicLibrary>,
    cache: std::collections::HashMap<CursorKind, Cursor>,
    xcursor: bool,
}

impl XcursorBackend {
    /// # Safety
    /// `display` must be a live Xlib display outliving the backend.
    /// Applies `XcursorSetTheme`/`XcursorSetSize` explicitly when available
    /// so the bundled theme/size bind even when process env arrives late.
    pub unsafe fn with_theme(
        display: *mut Display,
        theme: Option<&str>,
        size: Option<c_int>,
    ) -> Self {
        let loaded = DynamicLibrary::open(XCURSOR_LIBRARY_NAMES).ok();
        let load = loaded.as_ref().and_then(|lib| {
            unsafe { lib.symbol::<LibraryLoadCursorFn>(b"XcursorLibraryLoadCursor\0") }.ok()
        });
        if let Some(set_theme) = loaded.as_ref().and_then(|lib| {
            unsafe { lib.symbol::<LibrarySetThemeFn>(XCURSOR_SET_THEME_SYMBOL) }.ok()
        }) {
            if let Some(name) = theme.filter(|name| !name.is_empty()) {
                if let Ok(cname) = CString::new(name) {
                    unsafe { set_theme(display, cname.as_ptr()) };
                }
            }
        }
        if let Some(set_size) = loaded
            .as_ref()
            .and_then(|lib| unsafe { lib.symbol::<LibrarySetSizeFn>(XCURSOR_SET_SIZE_SYMBOL) }.ok())
        {
            if let Some(size) = size.filter(|size| *size > 0) {
                unsafe { set_size(display, size) };
            }
        }
        let xcursor = load.is_some();
        log_backend(if xcursor {
            "xcursor"
        } else {
            "core-font-cursor"
        });
        Self {
            display,
            load,
            _library: loaded,
            cache: std::collections::HashMap::new(),
            xcursor,
        }
    }

    pub fn uses_xcursor(&self) -> bool {
        self.xcursor
    }

    /// Resolve (and cache) a cursor for `kind`, trying Xcursor semantic
    /// names first, then the core font cursor fallback.
    ///
    /// # Safety
    /// `display` is the live display from `new`.
    pub unsafe fn cursor_for(&mut self, kind: CursorKind) -> Option<Cursor> {
        if let Some(cursor) = self.cache.get(&kind).copied() {
            return Some(cursor);
        }
        if let Some(load) = self.load {
            for name in cursor_names(kind) {
                let cname = CString::new(*name).ok()?;
                let cursor = unsafe { load(self.display, cname.as_ptr()) };
                if cursor != 0 {
                    self.cache.insert(kind, cursor);
                    return Some(cursor);
                }
            }
        }
        // Fallback: core font cursor via XCreateFontCursor.
        let cursor = unsafe { crate::xlib::XCreateFontCursor(self.display, core_shape(kind)) };
        if cursor != 0 {
            self.cache.insert(kind, cursor);
            return Some(cursor);
        }
        None
    }

    pub unsafe fn free_all(&mut self) {
        for (_, cursor) in self.cache.drain() {
            unsafe { crate::xlib::XFreeCursor(self.display, cursor) };
        }
    }

    /// Release cursor objects while the display is live, then transfer the
    /// library handle to the display lifetime owner.
    pub(crate) unsafe fn into_deferred_libraries(mut self) -> Vec<DynamicLibrary> {
        unsafe { self.free_all() };
        let library = self._library.take();
        drop(self);
        library.into_iter().collect()
    }

    /// Define the cursor for `kind` on `window`, falling back to Default.
    /// Callers pass the WM-computed semantic kind (resize edges, hover);
    /// this manager is the single renderer-side cursor owner.
    ///
    /// # Safety
    /// `window` must be live on this backend's display.
    pub unsafe fn define(&mut self, window: Window, kind: CursorKind) -> Option<Cursor> {
        let cursor = unsafe { self.cursor_for(kind) }
            .or_else(|| unsafe { self.cursor_for(CursorKind::Default) })?;
        unsafe {
            crate::xlib::XDefineCursor(self.display, window, cursor);
        }
        Some(cursor)
    }
}

fn core_shape(kind: CursorKind) -> u32 {
    match kind {
        CursorKind::Default => crate::xlib::XC_LEFT_PTR,
        CursorKind::Pointer => crate::xlib::XC_HAND2,
        CursorKind::Text => crate::xlib::XC_XTERM,
        CursorKind::Move => crate::xlib::XC_FLEUR,
        CursorKind::ResizeHorizontal => crate::xlib::XC_SB_H_DOUBLE_ARROW,
        CursorKind::ResizeVertical => crate::xlib::XC_SB_V_DOUBLE_ARROW,
        CursorKind::ResizeNorthWestSouthEast => crate::xlib::XC_BOTTOM_RIGHT_CORNER,
        CursorKind::ResizeNorthEastSouthWest => crate::xlib::XC_BOTTOM_LEFT_CORNER,
    }
}

fn cursor_names(kind: CursorKind) -> &'static [&'static str] {
    match kind {
        CursorKind::Default => &["default", "left_ptr"],
        CursorKind::Pointer => &["pointer", "hand2", "hand"],
        CursorKind::Text => &["text", "xterm", "ibeam"],
        CursorKind::Move => &["move", "fleur", "size_all"],
        CursorKind::ResizeHorizontal => &["h_double_arrow", "sb_h_double_arrow", "col-resize"],
        CursorKind::ResizeVertical => &["v_double_arrow", "sb_v_double_arrow", "row-resize"],
        CursorKind::ResizeNorthWestSouthEast => &["nwse-resize", "bottom_right_corner"],
        CursorKind::ResizeNorthEastSouthWest => &["nesw-resize", "bottom_left_corner"],
    }
}

/// Thin session facade over [`XcursorBackend`]: owns the backend lifetime.
pub struct NativeCursorSession {
    backend: XcursorBackend,
}

impl NativeCursorSession {
    #[must_use]
    pub fn wrap(backend: XcursorBackend) -> Self {
        Self { backend }
    }

    /// Define the cursor for `kind` on `window`.
    ///
    /// # Safety
    /// `window` must be live on this session's display.
    pub unsafe fn define(&mut self, window: Window, kind: CursorKind) -> Option<Cursor> {
        unsafe { self.backend.define(window, kind) }
    }

    pub unsafe fn free_all(&mut self) {
        unsafe { self.backend.free_all() };
    }

    pub(crate) unsafe fn into_deferred_libraries(self) -> Vec<DynamicLibrary> {
        unsafe { self.backend.into_deferred_libraries() }
    }
}

/// Dynamic backend handles retained by the display owner across
/// `XCloseDisplay`. Each backend first releases its display-bound objects while
/// the display is live; this owner then keeps the shared objects loaded until
/// Xlib has finished its close-display callbacks. This is the native lifetime
/// boundary: dropping a handle before XCloseDisplay is a use-after-unload risk.
pub(crate) struct DeferredNativeLibraryHandles {
    libraries: Vec<DynamicLibrary>,
}

impl DeferredNativeLibraryHandles {
    pub(crate) fn new() -> Self {
        Self {
            libraries: Vec::new(),
        }
    }

    /// # Safety
    /// All supplied backends refer to the same live display. Their X-backed
    /// resources are released before this owner is returned.
    pub(crate) unsafe fn from_backends(
        cursor: Option<NativeCursorSession>,
        xft: Option<XftBackend>,
        xrender: Option<XRenderBackend>,
        xshape: Option<XShapeBridge>,
    ) -> Self {
        let mut owner = Self::new();
        if let Some(cursor) = cursor {
            owner
                .libraries
                .extend(unsafe { cursor.into_deferred_libraries() });
        }
        if let Some(xft) = xft {
            owner
                .libraries
                .extend(unsafe { xft.into_deferred_libraries() });
        }
        if let Some(xrender) = xrender {
            owner
                .libraries
                .extend(unsafe { xrender.into_deferred_libraries() });
        }
        if let Some(xshape) = xshape {
            owner.libraries.extend(xshape.into_deferred_libraries());
        }
        owner
    }

    /// Merge handles from another display-bound owner before either owner is
    /// dropped. Both owners must belong to the same display lifetime.
    pub(crate) fn extend(&mut self, other: Self) {
        self.libraries.extend(other.libraries);
    }
}
