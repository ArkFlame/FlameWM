//! Border-only snap-preview and move/resize outline.
//!
//! One override-redirect window on the WM's own connection: its background pixel is the live
//! accent and its bounding shape is a ring, so the X server paints and restores it with no client
//! rendering, no backbuffer and no Expose handling. Input passes through (empty input shape).
//! A move only configures the window; the ring shape is rebuilt only when the size changes.
//! Snap targeting and geometry remain owned by `wm.rs`; this module owns only the window.

use flamewm_api::Rect;
use flamewm_window_core::SnapTarget;
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, PropMode,
    Rectangle, StackMode, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::live_settings;

/// Default fill opacity retained for the WM/settings interface. The border-only material
/// deliberately does not use opacity.
pub const DEFAULT_PREVIEW_OPACITY_PERCENT: u8 = 20;

/// Accept the settings projection; opacity cannot affect the border-only ring.
pub(crate) fn set_live_opacity(_opacity_percent: u8) {}

/// Ring width in device pixels.
const RING_BORDER_PX: u16 = 2;

/// `WM_NAME` of the ring window; the X11 canary finds the preview by it.
const WINDOW_NAME: &[u8] = b"FlameWM snap preview";

/// Opaque live system accent as a 24-bit `TrueColor` pixel.
fn accent_pixel() -> u32 {
    let (r, g, b) = live_settings::accent_rgb();
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

fn checked_size(value: i32) -> Option<u16> {
    u16::try_from(value).ok().filter(|size| *size > 0)
}

/// The ring as up to four window-local rectangles: top, bottom, left, right. A size thinner than
/// two borders is one filled rectangle.
#[must_use]
pub(crate) fn ring_rectangles(width: u16, height: u16) -> Vec<Rectangle> {
    let border = RING_BORDER_PX;
    if width <= 2 * border || height <= 2 * border {
        return vec![Rectangle {
            x: 0,
            y: 0,
            width,
            height,
        }];
    }
    let inner = height - 2 * border;
    vec![
        Rectangle {
            x: 0,
            y: 0,
            width,
            height: border,
        },
        Rectangle {
            x: 0,
            y: (height - border) as i16,
            width,
            height: border,
        },
        Rectangle {
            x: 0,
            y: border as i16,
            width: border,
            height: inner,
        },
        Rectangle {
            x: (width - border) as i16,
            y: border as i16,
            width: border,
            height: inner,
        },
    ]
}

/// Preview ring: hidden until a drag candidate or outline arrives.
pub struct SnapPreviewSurface {
    window: Option<Window>,
    visible: bool,
    candidate: Option<SnapTarget>,
    geometry: Option<Rect>,
    /// Size whose ring shape is currently applied.
    shape_size: Option<(u16, u16)>,
    /// Accent pixel currently set as the window background.
    pixel: Option<u32>,
}

impl SnapPreviewSurface {
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: None,
            visible: false,
            candidate: None,
            geometry: None,
            shape_size: None,
            pixel: None,
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

    /// True when candidate and geometry are unchanged and the ring is shown.
    #[must_use]
    pub fn is_current(&self, candidate: SnapTarget, geometry: Rect, _opacity: u8) -> bool {
        self.visible && self.candidate == Some(candidate) && self.geometry == Some(geometry)
    }

    /// Creates the ring window ahead of the first drag so the first outline is a single
    /// configure + map. Requests are only queued; nothing waits on the server.
    pub fn prepare(&mut self, conn: &impl Connection, root: Window) {
        if self.window.is_none() {
            self.window = create_window(conn, root);
        }
    }

    /// Show or move the preview for a live drag candidate. `None` hides.
    pub fn update(
        &mut self,
        conn: &impl Connection,
        root: Window,
        candidate: Option<SnapTarget>,
        geometry: Option<Rect>,
        _opacity: u8,
    ) {
        let (Some(candidate), Some(geometry)) = (candidate, geometry) else {
            self.hide(conn);
            return;
        };
        if self.is_current(candidate, geometry, DEFAULT_PREVIEW_OPACITY_PERCENT) {
            return;
        }
        self.candidate = Some(candidate);
        self.present(conn, root, geometry);
    }

    /// XP-style move/resize outline at `geometry`, on the same ring window as the snap preview.
    pub fn show_outline(&mut self, conn: &impl Connection, root: Window, geometry: Rect) {
        if self.visible && self.candidate.is_none() && self.geometry == Some(geometry) {
            return;
        }
        self.candidate = None;
        self.present(conn, root, geometry);
    }

    fn present(&mut self, conn: &impl Connection, root: Window, geometry: Rect) {
        let Some((width, height)) = checked_size(geometry.width).zip(checked_size(geometry.height))
        else {
            self.hide(conn);
            return;
        };
        self.prepare(conn, root);
        let Some(window) = self.window else {
            self.visible = false;
            return;
        };
        let mut aux = ConfigureWindowAux::new()
            .x(geometry.x)
            .y(geometry.y)
            .width(u32::from(width))
            .height(u32::from(height));
        if !self.visible {
            aux = aux.stack_mode(StackMode::ABOVE);
        }
        let pixel = accent_pixel();
        let mut ok = conn.configure_window(window, &aux).is_ok();
        if self.pixel != Some(pixel) {
            ok &= conn
                .change_window_attributes(
                    window,
                    &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                        .background_pixel(pixel),
                )
                .is_ok();
            if self.visible {
                ok &= conn.clear_area(false, window, 0, 0, 0, 0).is_ok();
            }
            self.pixel = Some(pixel);
        }
        if self.shape_size != Some((width, height)) {
            ok &= conn
                .shape_rectangles(
                    SO::SET,
                    SK::BOUNDING,
                    ClipOrdering::UNSORTED,
                    window,
                    0,
                    0,
                    &ring_rectangles(width, height),
                )
                .is_ok();
            self.shape_size = Some((width, height));
        }
        if !self.visible {
            ok &= conn.map_window(window).is_ok();
        }
        self.geometry = Some(geometry);
        self.visible = ok;
    }

    /// Hide the ring on release, cancel, unmanage, or shutdown.
    pub fn hide(&mut self, conn: &impl Connection) {
        if !self.visible && self.candidate.is_none() && self.geometry.is_none() {
            return;
        }
        if let Some(window) = self.window.filter(|_| self.visible) {
            let _ = conn.unmap_window(window);
        }
        self.visible = false;
        self.candidate = None;
        self.geometry = None;
    }
}

impl Default for SnapPreviewSurface {
    fn default() -> Self {
        Self::new()
    }
}

/// Override-redirect, input-transparent ring window. `None` when the requests cannot be queued.
fn create_window(conn: &impl Connection, root: Window) -> Option<Window> {
    let window = conn.generate_id().ok()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(accent_pixel())
            .border_pixel(0),
    )
    .ok()?;
    let _ = conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        WINDOW_NAME,
    );
    let _ = conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &[],
    );
    Some(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Rect {
        Rect::new(0, 0, 960, 1080)
    }

    #[test]
    fn ring_is_four_border_strips_covering_only_the_edges() {
        let rects = ring_rectangles(100, 50);
        assert_eq!(rects.len(), 4);
        let area: u32 = rects
            .iter()
            .map(|r| u32::from(r.width) * u32::from(r.height))
            .sum();
        assert_eq!(area, 100 * 50 - 96 * 46);
        let inside = |x: i16, y: i16| {
            rects.iter().any(|r| {
                x >= r.x && y >= r.y && x < r.x + r.width as i16 && y < r.y + r.height as i16
            })
        };
        assert!(inside(0, 0) && inside(99, 49) && inside(1, 25) && inside(98, 25));
        assert!(!inside(50, 25), "the ring centre is clear");
    }

    #[test]
    fn tiny_ring_is_one_filled_rectangle() {
        assert_eq!(ring_rectangles(3, 40).len(), 1);
        assert_eq!(ring_rectangles(40, 4).len(), 1);
    }

    #[test]
    fn border_follows_stored_accent() {
        let mut snapshot = flamewm_api::settings::SettingsSnapshot::new(1);
        snapshot.values.insert(
            "accent".into(),
            flamewm_api::settings::SettingValue::Text("#123456".into()),
        );
        crate::live_settings::sync(&snapshot);
        assert_eq!(accent_pixel(), 0x0012_3456);
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
    fn unchanged_candidate_geometry_is_current_independent_of_opacity() {
        let preview = SnapPreviewSurface {
            window: None,
            visible: true,
            candidate: Some(SnapTarget::LeftHalf),
            geometry: Some(geometry()),
            shape_size: None,
            pixel: None,
        };
        assert!(preview.is_current(SnapTarget::LeftHalf, geometry(), 20));
        assert!(preview.is_current(SnapTarget::LeftHalf, geometry(), 60));
        assert!(!preview.is_current(SnapTarget::RightHalf, geometry(), 20));
        assert!(!preview.is_current(SnapTarget::LeftHalf, Rect::new(960, 0, 960, 1080), 20));
    }
}
