//! Pseudo-transparency backdrop for a surface (taskbar, Start popup).
//!
//! No compositing manager exists, so a translucent scene pixel cannot blend with
//! the live desktop. The owner supplies a static ARGB snapshot of what is behind the
//! surface; it is laid into the scene first, and the translucent fills then blend
//! over it. The snapshot is not live: the owner refreshes it on show, move, resize
//! or wallpaper change. Without a backdrop the scene keeps its transparent clear.

use crate::xlib::*;

use super::X11App;

/// Debug trace for backdrop painting, enabled by `FLAMEWM_DEBUG_BACKDROP=1` (stderr).
fn trace_paint(message: &str) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_DEBUG_BACKDROP").is_some_and(|v| v == "1")) {
        eprintln!("debug backdrop paint {message}");
    }
}

/// Retained server pixmap holding one ARGB32 backdrop snapshot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceBackdrop {
    pub(crate) pixmap: Pixmap,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl X11App {
    /// Replaces the retained backdrop with `argb` (row-major 0xAARRGGBB, `width * height`
    /// words). The previous pixmap is freed only after the replacement is uploaded.
    ///
    /// # Safety
    ///
    /// The app's display must be live and this app must own its drawables.
    pub(crate) unsafe fn set_surface_backdrop(
        &mut self,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if width == 0 || height == 0 || argb.len() != width as usize * height as usize {
            return Err("surface backdrop: pixel count does not match size".to_string());
        }
        let mut vinfo: XVisualInfo = unsafe { std::mem::zeroed() };
        let matched =
            unsafe { XMatchVisualInfo(self.display, self.screen, 32, TRUE_COLOR_CLASS, &mut vinfo) };
        if matched == 0 || vinfo.visual.is_null() {
            return Err("surface backdrop: no 32-bit TrueColor visual".to_string());
        }
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let pixmap = unsafe { XCreatePixmap(self.display, root, width, height, 32) };
        if pixmap == 0 {
            return Err("surface backdrop: XCreatePixmap failed".to_string());
        }
        if let Err(error) = unsafe {
            self.upload_argb32_words(pixmap, vinfo.visual, argb, width, height)
        } {
            unsafe { XFreePixmap(self.display, pixmap) };
            return Err(error);
        }
        self.clear_surface_backdrop();
        self.surface_backdrop = Some(SurfaceBackdrop {
            pixmap,
            width,
            height,
        });
        Ok(())
    }

    /// Frees the retained backdrop, if any. Surfaces without backdrop keep the transparent clear.
    pub(crate) fn clear_surface_backdrop(&mut self) {
        if let Some(backdrop) = self.surface_backdrop.take() {
            unsafe { XFreePixmap(self.display, backdrop.pixmap) };
        }
    }

    /// Lays the retained backdrop over the freshly cleared scene (opaque OVER copy), so the
    /// translucent fills painted afterwards blend over the snapshot. No-op without backdrop
    /// or when the scene size no longer matches the snapshot.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread after `clear_scene`.
    pub(crate) unsafe fn paint_surface_backdrop(&mut self) -> Result<(), String> {
        let Some(backdrop) = self.surface_backdrop else {
            trace_paint("no-backdrop");
            return Ok(());
        };
        if backdrop.width != self.width || backdrop.height != self.height {
            trace_paint(&format!(
                "size-mismatch backdrop={}x{} scene={}x{}",
                backdrop.width, backdrop.height, self.width, self.height
            ));
            return Ok(());
        }
        let Some(xrender) = self.xrender.as_mut() else {
            trace_paint("no-xrender");
            return Ok(());
        };
        trace_paint(&format!("blit {}x{}", backdrop.width, backdrop.height));
        unsafe { xrender.blit_argb32_over(backdrop.pixmap, 0, 0, backdrop.width, backdrop.height) }
    }
}
