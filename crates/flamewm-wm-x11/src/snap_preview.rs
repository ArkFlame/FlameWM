//! Serialized, border-only snap-preview overlay.
//!
//! The preview is a compiled `snap-preview.html` document hosted on a
//! pass-through overlay surface. Snap targeting and geometry remain owned by
//! `wm.rs`; this module owns only the surface lifecycle. Since this renderer
//! has no verified desktop-alpha path, the entire gesture uses the deterministic
//! border-only material and never captures or installs backdrop pixels.

use flamewm_api::Rect;
use flamewm_render_x11::backdrop;
use flamewm_ui_x11::{
    SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiColor,
    UiDocumentAccess, UiTemplate, decode_document,
};
use flamewm_window_core::SnapTarget;

use crate::live_settings;

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-snap-preview.rwr"));

/// Default fill opacity retained for the WM/settings interface. The
/// border-only material deliberately does not use opacity.
pub const DEFAULT_PREVIEW_OPACITY_PERCENT: u8 = 20;

/// Accept the settings projection while fill rendering is unavailable. Opacity
/// cannot affect the deterministic border-only surface.
pub(crate) fn set_live_opacity(_opacity_percent: u8) {}

/// Ring width matches the `.snap-preview` CSS border (2px); the preview has no corner radius.
const RING_BORDER_PX: u32 = backdrop::BACKDROP_BORDER_WIDTH_PX;
const RING_RADIUS_PX: u32 = 0;

const FILL_NODE: &str = "snap-preview";
const BACKDROP_NODE: &str = "snap-preview-backdrop";

/// Opaque live system accent: with no compositor the ring is blended over the window's black
/// backing, so a translucent alpha would render as a dark tint instead of the accent.
fn border_color() -> UiColor {
    let (r, g, b) = live_settings::accent_rgb();
    UiColor { r, g, b, a: 0xff }
}

fn apply_border_only_style(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    document.visible(FILL_NODE, true)?;
    document.border(FILL_NODE, border_color())?;
    document.background_clear(FILL_NODE)?;
    document.visible(BACKDROP_NODE, false)?;
    Ok(())
}

fn checked_size(value: i32) -> Option<u32> {
    u32::try_from(value).ok().filter(|size| *size > 0)
}

/// Serialized preview surface: hidden until a drag candidate arrives.
pub struct SnapPreviewSurface {
    runtime: Option<SurfaceRuntime>,
    surface: Option<SurfaceHandle>,
    visible: bool,
    candidate: Option<SnapTarget>,
    geometry: Option<Rect>,
}

impl SnapPreviewSurface {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: None,
            surface: None,
            visible: false,
            candidate: None,
            geometry: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    #[must_use]
    pub fn candidate(&self) -> Option<SnapTarget> {
        self.candidate
    }

    #[cfg(test)]
    #[must_use]
    pub fn geometry(&self) -> Option<Rect> {
        self.geometry
    }

    /// True when candidate and geometry are unchanged and the surface is shown.
    #[must_use]
    pub fn is_current(&self, candidate: SnapTarget, geometry: Rect, _opacity: u8) -> bool {
        self.visible && self.candidate == Some(candidate) && self.geometry == Some(geometry)
    }

    /// Show or move the preview for a live drag candidate. `None` hides.
    /// Total without a display: records intent, leaves `visible` false.
    pub fn update(&mut self, candidate: Option<SnapTarget>, geometry: Option<Rect>, _opacity: u8) {
        let (Some(candidate), Some(geometry)) = (candidate, geometry) else {
            self.hide();
            return;
        };
        if self.is_current(candidate, geometry, DEFAULT_PREVIEW_OPACITY_PERCENT) {
            return;
        }
        let Some((width, height)) = checked_size(geometry.width).zip(checked_size(geometry.height))
        else {
            self.hide();
            return;
        };

        if self.candidate != Some(candidate) || self.geometry != Some(geometry) {
            self.hide();
            self.candidate = Some(candidate);
            self.geometry = Some(geometry);
        }
        self.present(geometry, width, height);
    }

    /// XP-style move/resize outline at `geometry`. Reuses the same border-only surface as the
    /// snap preview; while already visible only the position and size are updated.
    pub fn show_outline(&mut self, geometry: Rect) {
        if self.visible && self.candidate.is_none() && self.geometry == Some(geometry) {
            return;
        }
        let Some((width, height)) = checked_size(geometry.width).zip(checked_size(geometry.height))
        else {
            self.hide();
            return;
        };
        self.candidate = None;
        self.geometry = Some(geometry);
        self.present(geometry, width, height);
    }

    /// Move/size the overlay and make it visible. Shared by snap and outline paths.
    fn present(&mut self, geometry: Rect, width: u32, height: u32) {
        if !self.ensure_surface() {
            self.visible = false;
            return;
        }
        let (Some(runtime), Some(surface)) = (self.runtime.as_mut(), self.surface) else {
            self.visible = false;
            return;
        };
        if runtime
            .move_resize(surface, geometry.x, geometry.y, width, height)
            .is_err()
            || runtime
                .set_surface_ring_mask(surface, RING_BORDER_PX, RING_RADIUS_PX)
                .is_err()
        {
            self.hide();
            return;
        }
        if self.visible {
            if runtime.redraw(surface).is_err() {
                self.hide();
            }
            return;
        }

        let styled = runtime.with_document(surface, |document| apply_border_only_style(document));
        if styled.is_err() || runtime.redraw(surface).is_err() || runtime.show(surface).is_err() {
            self.hide();
            return;
        }
        let _ = runtime.raise(surface);
        self.visible = true;
    }

    /// Hide every preview node on release, cancel, unmanage, or shutdown.
    pub fn hide(&mut self) {
        if let (Some(runtime), Some(surface)) = (self.runtime.as_mut(), self.surface) {
            let _ = runtime.with_document(surface, |document| {
                document.visible(FILL_NODE, false)?;
                document.visible(BACKDROP_NODE, false)?;
                Ok(())
            });
            let _ = runtime.hide(surface);
        }
        self.visible = false;
        self.candidate = None;
        self.geometry = None;
    }

    /// Lazily create the overlay surface. `false` without a display.
    fn ensure_surface(&mut self) -> bool {
        if self.surface.is_some() {
            return true;
        }
        let mut runtime = match SurfaceRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return false,
        };
        let document = match decode_document(COMPILED_UI) {
            Ok(document) => document,
            Err(_) => return false,
        };
        let config = SurfaceConfig {
            width: 1,
            height: 1,
            title: "FlameWM snap preview".to_owned(),
            role: SurfaceRole::Overlay,
            input: SurfaceInputMode::PassThrough,
            initially_visible: false,
            x: 0,
            y: 0,
        };
        match runtime.create_surface(UiTemplate::new(document), config) {
            Ok(handle) => {
                // Ring mask before any paint or map: the first frame is never a black fill.
                if runtime
                    .set_surface_ring_mask(handle, RING_BORDER_PX, RING_RADIUS_PX)
                    .is_err()
                {
                    return false;
                }
                self.runtime = Some(runtime);
                self.surface = Some(handle);
                true
            }
            Err(_) => false,
        }
    }
}

impl Default for SnapPreviewSurface {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Rect {
        Rect::new(0, 0, 960, 1080)
    }

    #[test]
    fn snap_preview_css_default_is_neutral_not_red() {
        let css = include_str!("../ui/snap-preview.css").to_ascii_lowercase();
        assert!(
            !css.contains("ef4048"),
            "snap preview CSS still hard-codes the red accent"
        );
        assert!(
            css.contains("#7f878e"),
            "snap preview CSS default must be the neutral accent"
        );
    }

    #[test]
    fn snap_border_follows_stored_accent() {
        let mut snapshot = flamewm_api::settings::SettingsSnapshot::new(1);
        snapshot.values.insert(
            "accent".into(),
            flamewm_api::settings::SettingValue::Text("#123456".into()),
        );
        crate::live_settings::sync(&snapshot);
        let border = border_color();
        assert_eq!(
            (border.r, border.g, border.b, border.a),
            (0x12, 0x34, 0x56, 0xff)
        );
        crate::live_settings::sync(&flamewm_api::settings::SettingsSnapshot::new(1));
    }

    #[test]
    fn hidden_at_init() {
        let preview = SnapPreviewSurface::new();
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
        assert_eq!(preview.geometry(), None);
    }

    #[test]
    fn none_candidate_hides() {
        let mut preview = SnapPreviewSurface::new();
        preview.update(None, None, DEFAULT_PREVIEW_OPACITY_PERCENT);
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
    }

    #[test]
    fn unchanged_candidate_geometry_is_current_independent_of_opacity() {
        let preview = SnapPreviewSurface {
            runtime: None,
            surface: None,
            visible: true,
            candidate: Some(SnapTarget::LeftHalf),
            geometry: Some(geometry()),
        };
        assert!(preview.is_current(SnapTarget::LeftHalf, geometry(), 20));
        assert!(preview.is_current(SnapTarget::LeftHalf, geometry(), 60));
        assert!(!preview.is_current(SnapTarget::RightHalf, geometry(), 20));
        assert!(!preview.is_current(SnapTarget::LeftHalf, Rect::new(960, 0, 960, 1080), 20));
    }

    #[test]
    fn hide_clears_candidate_and_visibility() {
        let mut preview = SnapPreviewSurface {
            runtime: None,
            surface: None,
            visible: true,
            candidate: Some(SnapTarget::LeftHalf),
            geometry: Some(geometry()),
        };
        preview.hide();
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
        assert_eq!(preview.geometry(), None);
    }

    #[test]
    fn repeated_document_styling_keeps_deterministic_border_only_material() {
        let mut document = decode_document(COMPILED_UI).expect("compiled snap preview decodes");
        for cycle in 0..20 {
            apply_border_only_style(&mut document)
                .unwrap_or_else(|error| panic!("border-only document styling {cycle}: {error}"));
            assert_eq!(border_color().a, 0xff, "border material at cycle {cycle}");
        }
    }

    #[test]
    fn border_color_is_stable_and_does_not_depend_on_fill_opacity() {
        assert_eq!(border_color().a, 0xff);
        assert_eq!(DEFAULT_PREVIEW_OPACITY_PERCENT, 20);
    }
}
