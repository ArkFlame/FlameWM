use super::*;

pub(super) unsafe fn register_application_fonts(api: FontconfigApi) -> Result<(), String> {
    let Some(value) = env::var_os("FLAMEWM_RENDER_FONT_FILES") else {
        return Ok(());
    };
    // SAFETY: the config pointer is returned by fontconfig itself; NULL is rejected above.
    let config = unsafe { (api.config_get_current)() };
    if config.is_null() {
        return Err("FcConfigGetCurrent returned NULL".to_string());
    }
    let mut added = 0usize;
    for path in env::split_paths(&value) {
        if !path.is_file() {
            return Err(format!(
                "FLAMEWM_RENDER_FONT_FILES entry does not exist: {}",
                path.display()
            ));
        }
        let bytes = path_bytes(&path)?;
        let c_path = CString::new(bytes)
            .map_err(|_| format!("font path contains NUL: {}", path.display()))?;
        // SAFETY: `config` is non-NULL per the check above; `c_path` stays alive
        // for the call. A zero return is handled as an error below.
        if unsafe { (api.config_app_font_add_file)(config, c_path.as_ptr() as *const c_uchar) } == 0
        {
            return Err(format!(
                "FcConfigAppFontAddFile rejected {}",
                path.display()
            ));
        }
        added += 1;
    }
    // SAFETY: `config` is the non-NULL fontconfig handle checked above.
    if added > 0 && unsafe { (api.config_build_fonts)(config) } == 0 {
        return Err("FcConfigBuildFonts failed after adding application fonts".to_string());
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn path_bytes(path: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;
    Ok(path.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
pub(super) fn path_bytes(path: &Path) -> Result<Vec<u8>, String> {
    path.to_str()
        .map(|value| value.as_bytes().to_vec())
        .ok_or_else(|| format!("font path is not UTF-8: {}", path.display()))
}

pub(super) fn normalize_weight(weight: u16) -> u16 {
    match weight {
        0..=450 => 400,
        451..=550 => 500,
        551..=650 => 600,
        _ => 700,
    }
}

/// Runs `f` with layout widths measured by `xft` (`XftTextExtentsUtf8` at the requested weight).
/// With no Xft backend, layout falls back to the render-core model. Each call measures only the
/// text layout asks for; nothing is cached across frames, so the cost scales with layout work.
pub(crate) fn scoped_text_measure<R>(xft: Option<&mut XftBackend>, f: impl FnOnce() -> R) -> R {
    match xft {
        None => f(),
        Some(backend) => {
            let cell = std::cell::RefCell::new(backend);
            let measure = |text: &str, size: f32, weight: u16| {
                // SAFETY: the backend's display, fonts and draw surface live as long as the
                // surface, and `f` runs strictly inside this scope.
                unsafe { cell.borrow_mut().measure_text(size, weight, text).0 }
            };
            flamewm_render_core::text_layout::with_measure(&measure, f)
        }
    }
}

thread_local! {
    /// Per-thread Xft measurer for the process-wide default; opened on first measure, closed on
    /// thread exit. `None` when the display or Xft cannot open (the estimate is used then).
    static DEFAULT_XFT: std::cell::OnceCell<Option<DefaultXft>> = const { std::cell::OnceCell::new() };
}

/// Root-window measurer owned by one thread. Its display is never shared across threads.
pub(super) struct DefaultXft {
    pub(super) backend: std::cell::RefCell<Option<XftBackend>>,
    pub(super) display: *mut Display,
}

impl DefaultXft {
    pub(super) fn measure(&self, size: f32, weight: u16, text: &str) -> f32 {
        match self.backend.borrow_mut().as_mut() {
            // SAFETY: `backend` was built on `display`, which stays open until `Drop`.
            Some(backend) => unsafe { backend.measure_text(size, weight, text).0 },
            None => xft_measure_estimate(text, size).0,
        }
    }
}

impl Drop for DefaultXft {
    fn drop(&mut self) {
        if let Some(mut backend) = self.backend.get_mut().take() {
            // SAFETY: the display is still open; this frees the backend's Xft handles first.
            unsafe { backend.release_resources() };
            // shortcut: libXft/libXrender register Xlib close hooks that run inside XCloseDisplay,
            // so the backend's dlopen handles are leaked (once per thread exit); revisit when the
            // Xft symbols are linked directly or the hooks are unregistered before unload.
            std::mem::forget(backend);
        }
        // SAFETY: `display` was opened by `open_default_xft` on this thread and is closed once.
        unsafe { XCloseDisplay(self.display) };
    }
}

pub(super) fn open_default_xft() -> Option<DefaultXft> {
    // SAFETY: a NULL name opens `$DISPLAY`; each handle is checked before use and the root
    // window, visual and colormap belong to that display for its lifetime.
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return None;
        }
        let screen = XDefaultScreen(display);
        let root = XRootWindow(display, screen);
        let visual = XDefaultVisual(display, screen);
        let colormap = XDefaultColormap(display, screen);
        match XftBackend::new(display, screen, root, visual, colormap) {
            Ok(backend) => Some(DefaultXft {
                backend: std::cell::RefCell::new(Some(backend)),
                display,
            }),
            Err(_) => {
                XCloseDisplay(display);
                None
            }
        }
    }
}

pub(super) fn default_text_measure(text: &str, size: f32, weight: u16) -> f32 {
    DEFAULT_XFT.with(|cell| match cell.get_or_init(open_default_xft) {
        Some(xft) => xft.measure(size, weight, text),
        None => xft_measure_estimate(text, size).0,
    })
}

/// Installs Xft as the process-wide layout measure, so layout outside a scoped measure matches
/// the X11 paint. Each thread opens its own display lazily. First install wins.
pub fn install_default_layout_measure() -> bool {
    static MEASURE: fn(&str, f32, u16) -> f32 = default_text_measure;
    flamewm_render_core::text_layout::set_default_measure(&MEASURE)
}

/// Pure estimate backing [`XftBackend::measure_text`]: 0.58em per char,
/// 1.30em line height. Shared so the fallback path is unit-testable.
pub(crate) fn xft_measure_estimate(text: &str, size: f32) -> (f32, f32) {
    let height = (size.max(1.0) * 1.30).round();
    ((text.chars().count() as f32 * size * 0.58).round(), height)
}

/// Straight-alpha `Color` -> 16-bit XRender color triple used by
/// `XftColorAllocValue`. Pure so the alpha expansion is unit-testable.
pub(crate) fn xrender_color_for(color: Color) -> (u16, u16, u16, u16) {
    (
        u16::from(color.r) * 257,
        u16::from(color.g) * 257,
        u16::from(color.b) * 257,
        u16::from(color.a) * 257,
    )
}
