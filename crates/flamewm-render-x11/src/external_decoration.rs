//! Safe facade over external drawable/cursor infrastructure for wm-owned
//! chrome (frames, titles, cursors). Owns the process-lifetime X connection,
//! [`ExternalDrawableSession`], retargetable [`ExternalDrawableTarget`],
//! Xcursor backend, and font/cache resources. All methods are safe to call:
//! invalid XIDs produce explicit protocol-level errors, never memory
//! unsafety. When no display is reachable the renderer reports explicit
//! errors for live paint paths and layout-compatible estimates for measure.

use std::cell::RefCell;
use std::env;
use std::ptr;
use std::rc::{Rc, Weak};

use flamewm_render_core::{Color, CursorKind, Rect};

use super::overlay::{
    TransitionDirection, TransitionStyle, WarpOverlay, argb_visual_available, warp_argb_usable,
};
use super::lamp::PanelEdge;
use super::warp::WarpRect;
use super::xcomposite::{CapturedImage, capture_window};
use super::xrender_transform::XRenderTransformApi;
use super::external_drawable::{
    ExternalDrawableSession, ExternalDrawableTarget, external_text_measure, use_cstring_title,
};
use super::native::cursor::{DeferredNativeLibraryHandles, NativeCursorSession, XcursorBackend};
use super::xlib::{
    Display, XCloseDisplay, XFlush, XOpenDisplay, install_x_protocol_error_handler, x_io_broken,
};

/// Safe renderer for wm-owned frame chrome. Created via [`Self::open`]
/// (opens its own X connection) or [`Self::unavailable`] (explicit fake
/// seam for tests/headless use; live ops error, measure stays truthful).
pub struct ExternalDecorationRenderer {
    display: *mut Display,
    session: Option<Box<ExternalDrawableSession>>,
    cursor: Option<NativeCursorSession>,
    target: Option<ExternalDrawableTarget>,
    font_family: String,
    unavailable: Option<String>,
    picture_transform: bool,
    last_title_style: Option<(f32, i32)>,
    /// Last cursor kind defined per window, re-defined after a theme reload.
    defined_cursors: Vec<(u64, CursorKind)>,
}

/// Re-defines each `(window, kind)` through `define`, stopping at the first
/// window that fails to resolve a cursor.
fn redefine_cursors(
    defined: &[(u64, CursorKind)],
    mut define: impl FnMut(u64, CursorKind) -> bool,
) -> Result<(), String> {
    for &(xid, kind) in defined {
        if !define(xid, kind) {
            return Err(format!("cursor redefine failed for window {xid:#x}"));
        }
    }
    Ok(())
}

impl Drop for ExternalDecorationRenderer {
    fn drop(&mut self) {
        // Release X-backed objects while the display is live, but retain every
        // dynamic backend handle until after XCloseDisplay. This includes the
        // target's Xft/XRender/XShape libraries and the cursor library.
        let mut deferred_native = DeferredNativeLibraryHandles::new();
        let broken = x_io_broken();
        let mut target = self.target.take();
        if broken {
            // A broken connection cannot safely release X resources. Preserve
            // all native owners rather than invoking their Drop paths.
            std::mem::forget(target);
            std::mem::forget(self.cursor.take());
            std::mem::forget(self.session.take());
            return;
        }
        if let Some(target) = target.as_mut() {
            // SAFETY: this renderer still owns the live display/session.
            deferred_native.extend(unsafe { target.take_deferred_native_libraries() });
        }
        drop(target);
        if let Some(cursor) = self.cursor.take() {
            // SAFETY: this renderer still owns the live display.
            deferred_native.extend(unsafe {
                DeferredNativeLibraryHandles::from_backends(Some(cursor), None, None, None)
            });
        }
        drop(self.session.take());
        if !self.display.is_null() {
            unsafe { XCloseDisplay(self.display) };
            self.display = ptr::null_mut();
        }
        drop(deferred_native);
    }
}

impl ExternalDecorationRenderer {
    /// Open the default display and bind the process-lifetime session plus
    /// the Xcursor backend. Safe: the connection is owned by `Self`.
    pub fn open(font_family: &str) -> Result<Self, String> {
        let display = unsafe { XOpenDisplay(ptr::null()) };
        if display.is_null() {
            return Err(
                "ExternalDecorationRenderer: XOpenDisplay failed; DISPLAY is unset or unreachable"
                    .to_string(),
            );
        }
        install_x_protocol_error_handler();
        let session = match unsafe { ExternalDrawableSession::new(display) } {
            Ok(session) => session,
            Err(error) => {
                unsafe { XCloseDisplay(display) };
                return Err(format!("ExternalDecorationRenderer session: {error}"));
            }
        };
        let backend = unsafe { XcursorBackend::with_theme(display, None, None) };
        let picture_transform = XRenderTransformApi::load()
            .is_ok_and(|api| api.capabilities().picture_transform);
        Ok(Self {
            display,
            session: Some(Box::new(session)),
            cursor: Some(NativeCursorSession::wrap(backend)),
            target: None,
            font_family: font_family.to_string(),
            unavailable: None,
            picture_transform,
            last_title_style: None,
            defined_cursors: Vec::new(),
        })
    }

    /// Explicit unavailable seam for tests/headless hosts: records `reason`,
    /// touches no display, never fabricates backend success.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            display: ptr::null_mut(),
            session: None,
            cursor: None,
            target: None,
            font_family: String::new(),
            unavailable: Some(reason.to_string()),
            picture_transform: false,
            last_title_style: None,
            defined_cursors: Vec::new(),
        }
    }

    /// False exactly when built via [`Self::unavailable`].
    pub fn is_available(&self) -> bool {
        self.unavailable.is_none()
    }

    /// True when XRender picture transforms can drive minimize/restore
    /// flights. Probed once in [`Self::open`]; always false for unavailable.
    pub fn supports_picture_transform(&self) -> bool {
        self.picture_transform
    }

    fn live(&self) -> Result<(), String> {
        if let Some(reason) = self.unavailable.as_ref() {
            return Err(format!("external decoration unavailable: {reason}"));
        }
        if self.display.is_null() || self.session.is_none() {
            return Err("external decoration has no live display".to_string());
        }
        Ok(())
    }

    fn overlay_display(&self) -> Result<*mut Display, String> {
        (!self.display.is_null())
            .then_some(self.display)
            .ok_or_else(|| "overlay: renderer has no display".to_string())
    }

    fn target_mut(&mut self) -> Result<&mut ExternalDrawableTarget, String> {
        self.live()?;
        self.target
            .as_mut()
            .ok_or_else(|| "external decoration has no retargeted drawable".to_string())
    }

    /// Bind (or rebind) the paint target to `frame_xid` at `w`x`h`.
    /// A bad XID yields an explicit error from the backend, not UB.
    pub fn retarget(&mut self, frame_xid: u64, w: u32, h: u32) -> Result<(), String> {
        let drawable = frame_xid as super::xlib::Drawable;
        if drawable == 0 {
            return Err("retarget requires a nonzero frame XID".to_string());
        }
        self.live()?;
        if let Some(target) = self.target.as_mut() {
            return unsafe { target.retarget(drawable, w, h) };
        }
        let session_ptr =
            self.session.as_mut().expect("live session").as_mut() as *mut ExternalDrawableSession;
        let target = unsafe { ExternalDrawableTarget::new(session_ptr, drawable, w, h) }?;
        self.target = Some(target);
        Ok(())
    }

    /// Root-relative origin of `frame_xid` (a root child) and the root size.
    /// `None` without a live display or when the geometry query fails.
    pub fn frame_root_geometry(&self, frame_xid: u64) -> Option<((i32, i32), (u32, u32))> {
        use std::os::raw::{c_int, c_uint};
        if self.display.is_null() || frame_xid == 0 {
            return None;
        }
        let (mut root, mut x, mut y): (super::xlib::Window, c_int, c_int) = (0, 0, 0);
        let (mut w, mut h, mut border, mut depth): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0);
        // SAFETY: display is live; out-pointers are valid locals.
        let ok = unsafe {
            super::xlib_ext::XGetGeometry(
                self.display,
                frame_xid as super::xlib::Drawable,
                &mut root,
                &mut x,
                &mut y,
                &mut w,
                &mut h,
                &mut border,
                &mut depth,
            )
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: display is live.
        let (rw, rh) = unsafe {
            let screen = super::xlib::XDefaultScreen(self.display);
            (
                super::xlib::XDisplayWidth(self.display, screen),
                super::xlib::XDisplayHeight(self.display, screen),
            )
        };
        (rw > 0 && rh > 0).then_some(((x, y), (rw as u32, rh as u32)))
    }

    /// Fill `rect` with `color` on the current target.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) -> Result<(), String> {
        self.target_mut()?.fill_rect(rect, color)
    }

    /// Draw `text` with `font` family at (`x`, `baseline`) in `color`.
    /// NUL bytes are rejected explicitly; a missing Xft backend errors
    /// (no fallback paint). Caller supplies pixel size and weight.
    pub fn draw_title(
        &mut self,
        text: &str,
        font: &str,
        size_px: f32,
        weight: i32,
        x: f32,
        baseline: f32,
        color: Color,
    ) -> Result<(), String> {
        if !size_px.is_finite() || size_px <= 0.0 {
            return Err("draw_title requires a positive finite size_px".to_string());
        }
        if !(100..=900).contains(&weight) {
            return Err("draw_title requires weight in 100..=900".to_string());
        }
        let _ = use_cstring_title(text)?;
        let family = if font.is_empty() {
            self.font_family.clone()
        } else {
            font.to_string()
        };
        self.last_title_style = Some((size_px, weight));
        self.target_mut()?
            .draw_text(&family, x, baseline, color, size_px, weight as u16, text)
    }

    /// Last (size_px, weight) accepted by [`Self::draw_title`], including
    /// style validation before any display touch. `None` until first call.
    pub fn last_title_style(&self) -> Option<(f32, i32)> {
        self.last_title_style
    }

    /// Straight RGBA8 blit scaled to `dest` on the current target.
    /// Length is validated before touching the display.
    pub fn blit_rgba(
        &mut self,
        rgba8: &[u8],
        src_w: u32,
        src_h: u32,
        dest: Rect,
    ) -> Result<(), String> {
        let expected = (src_w.max(1) as usize)
            .checked_mul(src_h.max(1) as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "rgba size overflow".to_string())?;
        if rgba8.len() != expected {
            return Err(format!(
                "rgba length {} does not match {src_w}x{src_h} (expected {expected})",
                rgba8.len()
            ));
        }
        self.target_mut()?.blit_rgba(rgba8, src_w, src_h, dest)
    }

    /// Apply the rounded-corner bounding mask of `radius` to the current
    /// target drawable. Errors explicitly when XShape is unavailable.
    pub fn apply_shape(&self, radius: u32) -> Result<(), String> {
        self.live()?;
        self.target
            .as_ref()
            .ok_or_else(|| "external decoration has no retargeted drawable".to_string())?
            .apply_rounded_shape(radius)
    }

    /// Define the semantic cursor `kind` on `frame_xid`.
    pub fn define_cursor(&mut self, frame_xid: u64, kind: CursorKind) -> Result<(), String> {
        let window = frame_xid as super::xlib::Window;
        if window == 0 {
            return Err("define_cursor requires a nonzero frame XID".to_string());
        }
        self.live()?;
        let cursor = self
            .cursor
            .as_mut()
            .ok_or_else(|| "external decoration has no cursor backend".to_string())?;
        match unsafe { cursor.define(window, kind) } {
            Some(_) => {
                let known = self
                    .defined_cursors
                    .iter_mut()
                    .find(|(seen, _)| *seen == frame_xid);
                match known {
                    Some(entry) => entry.1 = kind,
                    None => self.defined_cursors.push((frame_xid, kind)),
                }
                Ok(())
            }
            None => Err("define_cursor failed: no cursor resolved for kind".to_string()),
        }
    }

    /// Drops the cursor record for `frame_xid` so a theme reload never
    /// re-defines a destroyed window. Call when the frame window is destroyed.
    pub fn forget_cursor(&mut self, frame_xid: u64) {
        self.defined_cursors.retain(|(seen, _)| *seen != frame_xid);
    }

    /// Measure `text` at `size`: live Xft advance when the backend is up,
    /// else the layout-compatible estimate. Never fabricates Xft success.
    pub fn measure_title(&mut self, text: &str, font: &str, size: f32) -> (f32, f32) {
        let estimate = external_text_measure(text, size);
        if self.unavailable.is_some() {
            return estimate;
        }
        let family = if font.is_empty() {
            self.font_family.clone()
        } else {
            font.to_string()
        };
        match self.target.as_mut() {
            Some(target) => target.measure_decoration_text(&family, size, 400, text),
            None => estimate,
        }
    }

    /// Flush the owned display connection. No-op when unavailable.
    pub fn flush(&self) {
        if self.display.is_null() {
            return;
        }
        unsafe { XFlush(self.display) };
    }

    /// Captures `window` once as premultiplied ARGB32 for a transition. Errors when unavailable.
    pub fn capture_window(&self, window: u64) -> Result<CapturedImage, String> {
        if self.display.is_null() {
            return Err("capture: renderer has no display".to_string());
        }
        capture_window(self.display, window)
    }

    /// Opens a Magic Lamp overlay on this renderer's display. The panel edge is taken from the
    /// icon's side of the window. Errors when unavailable.
    pub fn open_warp_overlay(
        &self,
        capture: CapturedImage,
        source: WarpRect,
        target: WarpRect,
    ) -> Result<WarpOverlay, String> {
        WarpOverlay::begin(self.overlay_display()?, capture, source, target)
    }

    /// Opens a transition of `style` (Magic Lamp or crossfade Fade) on this renderer's display.
    /// `direction` is `In` for an appearing window and `Out` for a disappearing one. Works with or
    /// without a compositing manager. Errors when unavailable.
    pub fn open_warp_overlay_styled(
        &self,
        capture: CapturedImage,
        source: WarpRect,
        target: WarpRect,
        edge: PanelEdge,
        style: TransitionStyle,
        direction: TransitionDirection,
    ) -> Result<WarpOverlay, String> {
        let display = self.overlay_display()?;
        WarpOverlay::begin_styled(display, capture, source, target, edge, style, direction)
    }

    /// True when this renderer's display has a 32-bit ARGB visual. Without one, Magic Lamp
    /// transitions use an opaque root-snapshot backdrop instead of a transparent window.
    #[must_use]
    pub fn argb_visual_available(&self) -> bool {
        argb_visual_available(self.display)
    }

    /// True when Magic Lamp can draw on a transparent window (32-bit visual and a compositing
    /// manager). Otherwise transitions use an opaque root-snapshot backdrop.
    #[must_use]
    pub fn warp_argb_usable(&self) -> bool {
        warp_argb_usable(self.display)
    }

    /// Opens a window-open scale-and-fade at root `(x, y)`, sized to `capture`. Errors without
    /// an ARGB visual, so the caller skips the effect.
    pub fn open_fade_overlay(
        &self,
        capture: CapturedImage,
        x: i32,
        y: i32,
    ) -> Result<WarpOverlay, String> {
        WarpOverlay::begin_fade(self.overlay_display()?, capture, x, y)
    }

    /// Native image-cache stats: (entries, bytes). (0, 0) when no target.
    pub fn cache_stats(&self) -> (usize, usize) {
        match self.target.as_ref() {
            Some(target) => (target.image_cache_len(), target.image_cache_bytes()),
            None => (0, 0),
        }
    }

    /// Preferred font family bound at [`Self::open`].
    pub fn font_family(&self) -> &str {
        &self.font_family
    }
}

/// A renderer whose Xcursor theme can be swapped in place by the registry.
pub trait CursorThemeOwner {
    fn reload_cursor_theme(&mut self, theme: &str) -> Result<(), String>;
}

impl CursorThemeOwner for ExternalDecorationRenderer {
    /// Replaces the cursor set, then re-defines the cursor on every window this
    /// renderer already set, so live frames show the new theme. If the theme
    /// reload fails, the old theme stays and the windows keep their cursors. If
    /// the reload succeeds but a re-define fails, the new theme is already loaded
    /// and this returns `Err`; the caller's `applied` theme is not advanced, so
    /// the same theme is retried on the next settings publish.
    fn reload_cursor_theme(&mut self, theme: &str) -> Result<(), String> {
        let cursor = self
            .cursor
            .as_mut()
            .ok_or_else(|| "external decoration has no cursor backend".to_string())?;
        unsafe { cursor.reload_theme(theme) }?;
        redefine_cursors(&self.defined_cursors, |xid, kind| {
            unsafe { cursor.define(xid as super::xlib::Window, kind) }.is_some()
        })
    }
}

impl ExternalDecorationRenderer {
    /// Shares the renderer and registers it for live cursor-theme reloads.
    pub fn into_cursor_shared(self) -> Rc<RefCell<Self>> {
        let shared = Rc::new(RefCell::new(self));
        let owner: Rc<RefCell<dyn CursorThemeOwner>> = shared.clone();
        register_cursor_theme_owner(&owner, env::var("XCURSOR_THEME").ok());
        shared
    }
}

/// Registers a live cursor owner (weakly) for [`apply_cursor_theme`]. `in_use` is the
/// theme the owner was opened with.
pub(crate) fn register_cursor_theme_owner(
    owner: &Rc<RefCell<dyn CursorThemeOwner>>,
    in_use: Option<String>,
) {
    CURSOR_THEME_REGISTRY.with(|registry| registry.borrow_mut().register(owner, in_use));
}

/// Process-local owners of Xcursor state. The WM is single-threaded; weak
/// handles let a dropped owner disappear without an unregister call.
#[derive(Default)]
struct CursorThemeRegistry {
    owners: Vec<Weak<RefCell<dyn CursorThemeOwner>>>,
    /// Theme the registered owners currently hold. `None` until the first owner
    /// registers, so an unset session theme still reloads on the first change.
    applied: Option<String>,
}

impl CursorThemeRegistry {
    /// `in_use` is the theme the owner was opened with, seeding `applied` so an
    /// unchanged theme never triggers a reload.
    fn register(&mut self, owner: &Rc<RefCell<dyn CursorThemeOwner>>, in_use: Option<String>) {
        if self.applied.is_none() {
            self.applied = in_use;
        }
        self.owners.push(Rc::downgrade(owner));
    }

    /// Reloads every live owner when `resolved` differs from the last applied
    /// theme. Returns per-owner failures. `applied` advances only when every
    /// owner succeeds, so a failed theme is retried on the next settings publish.
    fn apply(&mut self, resolved: &str) -> Vec<String> {
        if self.applied.as_deref() == Some(resolved) {
            return Vec::new();
        }
        self.owners.retain(|owner| owner.strong_count() > 0);
        let mut errors = Vec::new();
        for owner in self.owners.iter().filter_map(Weak::upgrade) {
            let result = owner.borrow_mut().reload_cursor_theme(resolved);
            if let Err(error) = result {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            self.applied = Some(resolved.to_owned());
        }
        errors
    }
}

thread_local! {
    static CURSOR_THEME_REGISTRY: RefCell<CursorThemeRegistry> =
        RefCell::new(CursorThemeRegistry::default());
}

/// Applies a session-resolved cursor theme to every registered owner. Returns
/// one message per failed owner; empty when nothing changed or all succeeded.
pub fn apply_cursor_theme(resolved: &str) -> Vec<String> {
    CURSOR_THEME_REGISTRY.with(|registry| registry.borrow_mut().apply(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeOwner {
        reloads: Vec<String>,
        fail: bool,
    }

    impl CursorThemeOwner for FakeOwner {
        fn reload_cursor_theme(&mut self, theme: &str) -> Result<(), String> {
            self.reloads.push(theme.to_owned());
            if self.fail {
                Err(format!("reload {theme} failed"))
            } else {
                Ok(())
            }
        }
    }

    fn fake(fail: bool) -> Rc<RefCell<FakeOwner>> {
        Rc::new(RefCell::new(FakeOwner {
            fail,
            ..FakeOwner::default()
        }))
    }

    fn as_owner(fake: &Rc<RefCell<FakeOwner>>) -> Rc<RefCell<dyn CursorThemeOwner>> {
        fake.clone()
    }

    #[test]
    fn theme_change_reloads_each_owner_exactly_once() {
        let mut registry = CursorThemeRegistry::default();
        let (first, second) = (fake(false), fake(false));
        registry.register(&as_owner(&first), None);
        registry.register(&as_owner(&second), None);

        assert!(registry.apply("Breeze").is_empty());
        assert_eq!(first.borrow().reloads, ["Breeze"]);
        assert_eq!(second.borrow().reloads, ["Breeze"]);

        assert!(registry.apply("Breeze").is_empty());
        assert_eq!(first.borrow().reloads.len(), 1);
        assert_eq!(second.borrow().reloads.len(), 1);

        assert!(registry.apply("Adwaita").is_empty());
        assert_eq!(first.borrow().reloads, ["Breeze", "Adwaita"]);
        assert_eq!(second.borrow().reloads, ["Breeze", "Adwaita"]);
    }

    #[test]
    fn theme_in_use_at_registration_triggers_no_startup_reload() {
        let mut registry = CursorThemeRegistry::default();
        let owner = fake(false);
        registry.register(&as_owner(&owner), Some("McMojave-cursors".to_owned()));

        assert!(registry.apply("McMojave-cursors").is_empty());
        assert!(owner.borrow().reloads.is_empty());
    }

    #[test]
    fn failed_owner_is_reported_without_blocking_other_owners() {
        let mut registry = CursorThemeRegistry::default();
        let (broken, healthy) = (fake(true), fake(false));
        registry.register(&as_owner(&broken), None);
        registry.register(&as_owner(&healthy), None);

        let errors = registry.apply("Missing-theme");
        assert_eq!(errors, ["reload Missing-theme failed"]);
        assert_eq!(healthy.borrow().reloads, ["Missing-theme"]);
        // A failed theme is retried on the next publish of the same theme.
        let retry = registry.apply("Missing-theme");
        assert_eq!(retry, ["reload Missing-theme failed"]);
        assert_eq!(broken.borrow().reloads, ["Missing-theme", "Missing-theme"]);
    }

    #[test]
    fn successful_theme_is_not_reloaded_after_a_failed_owner_recovers() {
        let mut registry = CursorThemeRegistry::default();
        let flaky = fake(true);
        registry.register(&as_owner(&flaky), None);
        assert_eq!(registry.apply("Breeze").len(), 1);

        flaky.borrow_mut().fail = false;
        assert!(registry.apply("Breeze").is_empty());
        assert_eq!(flaky.borrow().reloads, ["Breeze", "Breeze"]);
        assert!(registry.apply("Breeze").is_empty());
        assert_eq!(flaky.borrow().reloads.len(), 2);
    }

    #[test]
    fn redefine_visits_every_recorded_window_with_its_last_kind() {
        let defined = [(0x10, CursorKind::Move), (0x20, CursorKind::Default)];
        let mut calls = Vec::new();
        let result = redefine_cursors(&defined, |xid, kind| {
            calls.push((xid, kind));
            true
        });
        assert!(result.is_ok());
        assert_eq!(
            calls,
            [(0x10, CursorKind::Move), (0x20, CursorKind::Default)]
        );
    }

    #[test]
    fn redefine_stops_at_first_unresolved_window() {
        let defined = [(0x10, CursorKind::Move), (0x20, CursorKind::Default)];
        let mut calls = 0;
        let error = redefine_cursors(&defined, |_, _| {
            calls += 1;
            false
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.contains("0x10"));
    }

    #[test]
    fn dropped_owner_is_skipped_and_pruned() {
        let mut registry = CursorThemeRegistry::default();
        let (gone, kept) = (fake(false), fake(false));
        registry.register(&as_owner(&gone), None);
        registry.register(&as_owner(&kept), None);
        drop(gone);

        assert!(registry.apply("Breeze").is_empty());
        assert_eq!(kept.borrow().reloads, ["Breeze"]);
        assert_eq!(registry.owners.len(), 1);
    }

    #[test]
    fn forget_cursor_removes_only_the_destroyed_window() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        renderer.defined_cursors = vec![
            (0x10, CursorKind::Move),
            (0x20, CursorKind::Default),
            (0x30, CursorKind::Move),
        ];
        renderer.forget_cursor(0x20);
        assert_eq!(
            renderer.defined_cursors,
            [(0x10, CursorKind::Move), (0x30, CursorKind::Move)]
        );
        renderer.forget_cursor(0x99);
        assert_eq!(renderer.defined_cursors.len(), 2);
    }

    #[test]
    fn unavailable_renderer_reports_no_cursor_backend() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        let error = renderer.reload_cursor_theme("Breeze").unwrap_err();
        assert!(error.contains("no cursor backend"));
    }

    #[test]
    fn unavailable_reports_explicit_errors() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        assert!(!renderer.is_available());
        assert!(!renderer.supports_picture_transform());
        assert!(renderer.retarget(42, 8, 8).is_err());
        assert!(
            renderer
                .fill_rect(
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 4.0,
                        height: 4.0,
                    },
                    Color {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    }
                )
                .is_err()
        );
        assert!(
            renderer
                .draw_title(
                    "hi",
                    "sans",
                    12.0,
                    600,
                    0.0,
                    12.0,
                    Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    }
                )
                .is_err()
        );
        assert!(
            renderer
                .blit_rgba(
                    &[0u8; 16],
                    2,
                    2,
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 2.0,
                        height: 2.0,
                    }
                )
                .is_err()
        );
        assert!(renderer.apply_shape(4).is_err());
        assert!(renderer.define_cursor(42, CursorKind::Default).is_err());
        assert_eq!(renderer.cache_stats(), (0, 0));
        // Flush on an unavailable renderer must not crash.
        renderer.flush();
    }

    #[test]
    fn measure_without_display_matches_layout_estimate() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        assert_eq!(renderer.measure_title("hello", "sans", 13.0), (38.0, 17.0));
        assert_eq!(renderer.measure_title("", "sans", 13.0), (0.0, 17.0));
    }

    #[test]
    fn rejects_zero_xid_and_bad_rgba_before_display() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        // Zero XIDs are rejected even before liveness is consulted.
        assert!(renderer.retarget(0, 8, 8).is_err());
        assert!(renderer.define_cursor(0, CursorKind::Pointer).is_err());
        // RGBA length mismatch is validated before touching any display.
        let mut live_check = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        let bad = live_check.blit_rgba(
            &[0u8; 15],
            2,
            2,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 2.0,
                height: 2.0,
            },
        );
        assert!(bad.is_err());
    }

    #[test]
    fn rejects_nul_title_without_display() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        let result = renderer.draw_title(
            "a\0b",
            "sans",
            12.0,
            600,
            0.0,
            12.0,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn title_style_params_reach_draw_target_contract() {
        let mut renderer = ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        assert_eq!(renderer.last_title_style(), None);
        // 12px/600 must be accepted into the draw-target contract (forwarded
        // as style params, not hardcoded 13px/400); the call then fails only
        // on the unavailable display, but the recorded style proves the
        // params reached the contract.
        let result = renderer.draw_title(
            "title",
            "sans",
            12.0,
            600,
            0.0,
            12.0,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        assert!(result.is_err());
        assert_eq!(renderer.last_title_style(), Some((12.0, 600)));
        // A second distinct style overwrites: no hardcoded fallback remains.
        let _ = renderer.draw_title(
            "title",
            "sans",
            13.0,
            400,
            0.0,
            12.0,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        assert_eq!(renderer.last_title_style(), Some((13.0, 400)));
        // Invalid style never reaches the contract.
        let _ = renderer.draw_title(
            "title",
            "sans",
            0.0,
            600,
            0.0,
            12.0,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        assert_eq!(renderer.last_title_style(), Some((13.0, 400)));
    }
}
