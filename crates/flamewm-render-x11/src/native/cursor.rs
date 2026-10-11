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
    set_theme: Option<LibrarySetThemeFn>,
    theme: Option<String>,
    _library: Option<DynamicLibrary>,
    cache: std::collections::HashMap<CursorKind, Cursor>,
    xcursor: bool,
}

/// Cursor-object source used by [`reload_cache`]. The Xlib implementation is
/// [`XlibCursorLoader`]; tests substitute a fake to observe frees exactly.
pub(crate) trait CursorLoader {
    /// Make `theme` the active Xcursor theme for later `load` calls.
    fn set_theme(&mut self, theme: &str);
    /// Create a new owned cursor for `name`; `0` when the theme lacks it.
    fn load(&mut self, name: &str) -> Cursor;
    /// Release one owned cursor object.
    fn free(&mut self, cursor: Cursor);
}

struct XlibCursorLoader {
    display: *mut Display,
    load: LibraryLoadCursorFn,
    set_theme: Option<LibrarySetThemeFn>,
}

impl CursorLoader for XlibCursorLoader {
    fn set_theme(&mut self, theme: &str) {
        if let (Some(set_theme), Ok(cname)) = (self.set_theme, CString::new(theme)) {
            unsafe { set_theme(self.display, cname.as_ptr()) };
        }
    }

    fn load(&mut self, name: &str) -> Cursor {
        match CString::new(name) {
            Ok(cname) => unsafe { (self.load)(self.display, cname.as_ptr()) },
            Err(_) => 0,
        }
    }

    fn free(&mut self, cursor: Cursor) {
        unsafe { crate::xlib::XFreeCursor(self.display, cursor) };
    }
}

const ALL_CURSOR_KINDS: [CursorKind; 8] = [
    CursorKind::Default,
    CursorKind::Pointer,
    CursorKind::Text,
    CursorKind::Move,
    CursorKind::ResizeHorizontal,
    CursorKind::ResizeVertical,
    CursorKind::ResizeNorthWestSouthEast,
    CursorKind::ResizeNorthEastSouthWest,
];

/// Build a complete cursor set for `theme` before touching `cache`.
///
/// Every kind must resolve through Xcursor names under the new theme. On any
/// miss the partially built set is freed, the previous theme is restored, and
/// `cache` is untouched. On success the old set is freed exactly once and
/// replaced. Cursors still referenced by windows stay valid on the X server
/// until those windows re-define their cursor.
pub(crate) fn reload_cache<L: CursorLoader>(
    loader: &mut L,
    cache: &mut std::collections::HashMap<CursorKind, Cursor>,
    previous_theme: Option<&str>,
    theme: &str,
) -> Result<(), String> {
    loader.set_theme(theme);
    let mut fresh = std::collections::HashMap::with_capacity(ALL_CURSOR_KINDS.len());
    for kind in ALL_CURSOR_KINDS {
        let cursor = cursor_names(kind)
            .iter()
            .map(|name| loader.load(name))
            .find(|cursor| *cursor != 0);
        match cursor {
            Some(cursor) => {
                fresh.insert(kind, cursor);
            }
            None => {
                for (_, built) in fresh.drain() {
                    loader.free(built);
                }
                if let Some(previous) = previous_theme {
                    loader.set_theme(previous);
                }
                return Err(format!("cursor theme {theme} lacks {kind:?}"));
            }
        }
    }
    for (_, old) in cache.drain() {
        loader.free(old);
    }
    cache.extend(fresh);
    Ok(())
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
        let set_theme = loaded.as_ref().and_then(|lib| {
            unsafe { lib.symbol::<LibrarySetThemeFn>(XCURSOR_SET_THEME_SYMBOL) }.ok()
        });
        Self {
            display,
            load,
            set_theme,
            theme: theme.filter(|name| !name.is_empty()).map(str::to_owned),
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

    /// Replace the Xcursor theme in place. On error the previous theme and
    /// cursor set stay active. Callers must re-define cursors on their windows
    /// after success (cached cursor IDs are replaced).
    ///
    /// # Safety
    /// `display` is the live display from `with_theme`.
    pub unsafe fn reload_theme(&mut self, theme: &str) -> Result<(), String> {
        let Some(load) = self.load.filter(|_| self.xcursor) else {
            return Err("Xcursor library unavailable".to_owned());
        };
        if theme.is_empty() {
            return Err("empty cursor theme".to_owned());
        }
        let mut loader = XlibCursorLoader {
            display: self.display,
            load,
            set_theme: self.set_theme,
        };
        reload_cache(&mut loader, &mut self.cache, self.theme.as_deref(), theme)?;
        self.theme = Some(theme.to_owned());
        Ok(())
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

    /// # Safety
    /// The session's display must be live.
    pub unsafe fn reload_theme(&mut self, theme: &str) -> Result<(), String> {
        unsafe { self.backend.reload_theme(theme) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Fake theme: `missing` names never load; every successful load gets a
    /// fresh handle, and frees are recorded per handle.
    #[derive(Default)]
    struct FakeLoader {
        theme: String,
        next: Cursor,
        missing: Vec<String>,
        frees: Vec<Cursor>,
        themes_loaded: HashMap<String, Cursor>,
    }

    impl CursorLoader for FakeLoader {
        fn set_theme(&mut self, theme: &str) {
            self.theme = theme.to_owned();
        }

        fn load(&mut self, name: &str) -> Cursor {
            if self.missing.iter().any(|missing| missing == name) || self.theme == "broken" {
                return 0;
            }
            self.next += 1;
            self.themes_loaded.insert(name.to_owned(), self.next);
            self.next
        }

        fn free(&mut self, cursor: Cursor) {
            self.frees.push(cursor);
        }
    }

    fn seeded_cache(loader: &mut FakeLoader, theme: &str) -> HashMap<CursorKind, Cursor> {
        let mut cache = HashMap::new();
        reload_cache(loader, &mut cache, None, theme).expect("initial theme");
        loader.frees.clear();
        cache
    }

    #[test]
    fn theme_switch_frees_old_cursors_exactly_once() {
        let mut loader = FakeLoader::default();
        let mut cache = seeded_cache(&mut loader, "old");
        let old: Vec<Cursor> = cache.values().copied().collect();
        reload_cache(&mut loader, &mut cache, Some("old"), "new").expect("switch");
        let mut frees = loader.frees.clone();
        frees.sort_unstable();
        let mut expected = old.clone();
        expected.sort_unstable();
        assert_eq!(frees, expected, "each old handle freed once, none twice");
        assert_eq!(cache.len(), ALL_CURSOR_KINDS.len());
        assert!(cache.values().all(|cursor| !old.contains(cursor)));
        assert_eq!(loader.theme, "new");
    }

    #[test]
    fn broken_theme_keeps_previous_cursors_and_theme() {
        let mut loader = FakeLoader::default();
        let mut cache = seeded_cache(&mut loader, "old");
        let before = cache.clone();
        loader.missing = vec!["h_double_arrow".into(), "sb_h_double_arrow".into(), "col-resize".into()];
        let error = reload_cache(&mut loader, &mut cache, Some("old"), "partial")
            .expect_err("partial theme must be rejected");
        assert!(error.contains("ResizeHorizontal"), "{error}");
        assert_eq!(cache, before, "previous cursor set untouched");
        assert!(loader.frees.iter().all(|cursor| !before.values().any(|old| old == cursor)));
        assert_eq!(loader.theme, "old", "previous theme restored");
    }

    #[test]
    fn unknown_theme_is_rejected_without_frees() {
        let mut loader = FakeLoader::default();
        let mut cache = seeded_cache(&mut loader, "old");
        let before = cache.clone();
        reload_cache(&mut loader, &mut cache, Some("old"), "broken")
            .expect_err("unloadable theme");
        assert_eq!(cache, before);
        assert!(loader.frees.is_empty(), "nothing was built, so nothing freed");
    }
}
