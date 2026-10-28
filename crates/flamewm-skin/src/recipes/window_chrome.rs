use crate::icons::IconRole;
use crate::palette::{PANEL, Rgb};
use crate::typography::{TextStyle, Typography};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControlRole {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl WindowControlRole {
    /// Breeze asset role for this control (`assets/web/breeze/window-*.svg`).
    #[must_use]
    pub const fn icon_role(self) -> IconRole {
        match self {
            Self::Minimize => IconRole::Minimize,
            Self::Maximize => IconRole::Maximize,
            Self::Restore => IconRole::Restore,
            Self::Close => IconRole::Close,
        }
    }
}

/// Renderer-neutral pixel rectangle (no native types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl SceneRect {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Pure window chrome scene: bounds, title, style, icon/controls, pointer and state flags.
/// No rendering, no native calls, no WM policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowChromeScene {
    pub bounds: SceneRect,
    pub title: String,
    pub title_style: TextStyle,
    pub app_icon: Option<IconRole>,
    pub control_roles: [WindowControlRole; 3],
    pub hover: Option<WindowControlRole>,
    pub pressed: Option<WindowControlRole>,
    pub active: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl WindowChromeScene {
    #[must_use]
    pub fn new(bounds: SceneRect, title: impl Into<String>) -> Self {
        Self {
            bounds,
            title: title.into(),
            title_style: Typography::RWR_0_0_9.title,
            app_icon: None,
            control_roles: WINDOW_CHROME.controls,
            hover: None,
            pressed: None,
            active: true,
            maximized: false,
            fullscreen: false,
        }
    }

    /// Titlebar strip at the top of `bounds`.
    #[must_use]
    pub fn titlebar_rect(&self) -> SceneRect {
        SceneRect::new(
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            i32::from(WINDOW_CHROME.metrics.titlebar),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowChromeRecipe {
    pub metrics: crate::metrics::WindowChromeMetrics,
    pub controls: [WindowControlRole; 3],
    pub titlebar_background: Rgb,
    pub border: Rgb,
    pub close_hover: Rgb,
}

/// CSS `.window`, `.titlebar`, `.title-side`, `.window-controls`, and title-button rules.
pub const WINDOW_CHROME: WindowChromeRecipe = WindowChromeRecipe {
    metrics: crate::DEFAULT.chrome,
    controls: [
        WindowControlRole::Minimize,
        WindowControlRole::Maximize,
        WindowControlRole::Close,
    ],
    titlebar_background: PANEL,
    border: Rgb(0x0046_484b),
    close_hover: Rgb(0x00e8_1123),
};

/// Vertical padding inside the titlebar reserved above/below the icon slot.
pub const ICON_SLOT_PAD_Y: u16 = 4;
/// Minimum icon slot edge when the titlebar is short.
pub const ICON_SLOT_MIN: u16 = 8;
/// Left padding before the icon slot.
pub const ICON_PAD_LEFT: i32 = 6;
/// Padding between the occupied edges and the centered title.
pub const TITLE_PAD: i32 = 6;

/// Button geometry for one titlebar control, left-to-right index 0..=2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlGeometry {
    pub role: WindowControlRole,
    pub bounds: SceneRect,
}

/// Total width occupied by the three titlebar controls.
#[must_use]
pub const fn controls_width() -> i32 {
    WINDOW_CHROME.metrics.button_width as i32 * 3
}

/// Square icon slot edge, vertically centered: `min(icon, titlebar - pad)`, floored at min.
#[must_use]
pub const fn icon_slot_edge() -> u16 {
    let metrics = crate::DEFAULT.chrome;
    let room = metrics.titlebar.saturating_sub(ICON_SLOT_PAD_Y);
    let edge = if room < metrics.icon {
        room
    } else {
        metrics.icon
    };
    if edge < ICON_SLOT_MIN {
        ICON_SLOT_MIN
    } else {
        edge
    }
}

/// X offsets of the control buttons from the right frame edge (index 0 = leftmost).
#[must_use]
pub fn control_button_geometries(titlebar: SceneRect) -> [ControlGeometry; 3] {
    let width = i32::from(WINDOW_CHROME.metrics.button_width);
    let height = i32::from(WINDOW_CHROME.metrics.titlebar);
    let roles = WINDOW_CHROME.controls;
    [0, 1, 2].map(|index: usize| {
        let from_right = 2 - index as i32;
        let x = titlebar.x + titlebar.width - width * (from_right + 1);
        ControlGeometry {
            role: roles[index],
            bounds: SceneRect::new(x, titlebar.y, width, height),
        }
    })
}

/// Fallback titlebar-control artwork shared by X11 and Wayland. The SVG icon theme may be
/// absent on minimal installs; this small antialiased raster preserves recognizable controls
/// without a runtime icon dependency. Pixels are straight RGBA8 (not premultiplied).
/// The caller chooses a material/foreground color and composites it using its native backend.
#[must_use]
pub fn control_glyph_rgba(role: WindowControlRole, edge: u32, rgb: u32) -> Option<Vec<u8>> {
    if !(4..=128).contains(&edge) {
        return None;
    }
    let side = edge as f32;
    let scale = side / 16.0;
    let center = (side - 1.0) * 0.5;
    let at = |x: f32, y: f32| (center + x * scale, center + y * scale);
    let mut strokes = Vec::with_capacity(8);
    let mut segment = |ax: f32, ay: f32, bx: f32, by: f32| {
        strokes.push((at(ax, ay), at(bx, by)));
    };
    match role {
        WindowControlRole::Minimize => segment(-5.0, 4.0, 5.0, 4.0),
        WindowControlRole::Maximize => {
            segment(-5.0, -5.0, 5.0, -5.0);
            segment(5.0, -5.0, 5.0, 5.0);
            segment(5.0, 5.0, -5.0, 5.0);
            segment(-5.0, 5.0, -5.0, -5.0);
        }
        WindowControlRole::Restore => {
            segment(-4.0, -2.0, 4.0, -2.0);
            segment(4.0, -2.0, 4.0, 5.0);
            segment(4.0, 5.0, -4.0, 5.0);
            segment(-4.0, 5.0, -4.0, -2.0);
            segment(-2.0, -4.0, 5.0, -4.0);
            segment(5.0, -4.0, 5.0, 3.0);
        }
        WindowControlRole::Close => {
            segment(-5.0, -5.0, 5.0, 5.0);
            segment(-5.0, 5.0, 5.0, -5.0);
        }
    }
    let mut pixels = vec![0_u8; edge as usize * edge as usize * 4];
    for y in 0..edge {
        for x in 0..edge {
            let px = x as f32;
            let py = y as f32;
            let distance = strokes
                .iter()
                .map(|&(a, b)| {
                    let vx = b.0 - a.0;
                    let vy = b.1 - a.1;
                    let length2 = vx * vx + vy * vy;
                    let t = if length2 > 0.0 {
                        ((px - a.0) * vx + (py - a.1) * vy) / length2
                    } else {
                        0.0
                    }
                    .clamp(0.0, 1.0);
                    let dx = px - (a.0 + t * vx);
                    let dy = py - (a.1 + t * vy);
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(f32::INFINITY, f32::min);
            let alpha = ((1.1 * scale - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
            let index = (y as usize * edge as usize + x as usize) * 4;
            pixels[index] = (rgb >> 16) as u8;
            pixels[index + 1] = (rgb >> 8) as u8;
            pixels[index + 2] = rgb as u8;
            pixels[index + 3] = alpha;
        }
    }
    Some(pixels)
}

/// Available title text width after respecting the left icon and right control strip.
/// Both native backends must shorten the title *before* computing its centered origin;
/// center_title_x alone cannot prevent overlap when the title exceeds this space.
#[must_use]
pub fn title_available_width(left_occupied: i32, right_occupied: i32, padding: i32) -> i32 {
    right_occupied
        .saturating_sub(left_occupied)
        .saturating_sub(padding.saturating_mul(2))
        .max(0)
}

/// A backend supplies its font's measured/estimated advance for each character. Keep the
/// ellipsis inside the available width so it never paints across close/maximize controls.
#[must_use]
pub fn elide_title(
    title: &str,
    available_width: i32,
    mut advance: impl FnMut(char) -> i32,
) -> String {
    let capacity = available_width.max(0);
    let mut result = String::new();
    let mut width = 0_i32;
    for ch in title.chars().take(256) {
        let next = advance(ch).max(0);
        if width.saturating_add(next) > capacity {
            let dots = 3_i32.saturating_mul(advance('.').max(0));
            if dots > capacity {
                return result;
            }
            while !result.is_empty() && width.saturating_add(dots) > capacity {
                if let Some(removed) = result.pop() {
                    width = width.saturating_sub(advance(removed).max(0));
                }
            }
            if dots <= capacity && width.saturating_add(dots) <= capacity {
                result.push_str("...");
            }
            return result;
        }
        result.push(ch);
        width = width.saturating_add(next);
    }
    result
}

/// Center-title formula: `desired = (bar_w - title_w) / 2` clamped into
/// `[left_occupied + pad, right_occupied - pad - title_w]` with sorted bounds.
#[must_use]
pub fn center_title_x(
    titlebar_width: i32,
    title_text_width: i32,
    left_occupied: i32,
    right_occupied: i32,
    padding: i32,
) -> i32 {
    let desired = titlebar_width.saturating_sub(title_text_width) / 2;
    let minimum = left_occupied.saturating_add(padding);
    let maximum = right_occupied
        .saturating_sub(padding)
        .saturating_sub(title_text_width);
    desired.clamp(minimum.min(maximum), maximum.max(minimum))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_glyphs_render_all_four_semantics_without_heavy_assets() {
        for role in [
            WindowControlRole::Minimize,
            WindowControlRole::Maximize,
            WindowControlRole::Restore,
            WindowControlRole::Close,
        ] {
            let pixels = control_glyph_rgba(role, 16, 0x00_f1_f2_f3).unwrap();
            assert_eq!(pixels.len(), 16 * 16 * 4);
            assert!(pixels.chunks_exact(4).any(|px| px[3] > 0));
            assert_eq!(pixels[3], 0, "icon corner must stay transparent");
            assert_eq!(&pixels[0..3], &[0xf1, 0xf2, 0xf3]);
        }
        assert!(control_glyph_rgba(WindowControlRole::Close, 0, 0).is_none());
        assert!(control_glyph_rgba(WindowControlRole::Close, 129, 0).is_none());
        assert_ne!(
            control_glyph_rgba(WindowControlRole::Maximize, 16, 0),
            control_glyph_rgba(WindowControlRole::Restore, 16, 0),
        );
    }

    #[test]
    fn center_formula_matches_flame_contract() {
        // desired=(400-90)/2=155 inside free region.
        assert_eq!(center_title_x(400, 90, 22, 304, TITLE_PAD), 155);
        // Desired below minimum but free region inverted: sorted clamp keeps desired (20).
        assert_eq!(center_title_x(400, 360, 22, 304, TITLE_PAD), 20);
        // Title wider than the free region: sorted clamp keeps desired (10).
        assert_eq!(center_title_x(200, 180, 22, 104, TITLE_PAD), 10);
    }

    #[test]
    fn long_title_cannot_intrude_into_control_strip() {
        let available = title_available_width(22, 304, TITLE_PAD);
        assert_eq!(available, 270);
        let text = elide_title(&"A".repeat(100), available, |_| 7);
        assert!(text.ends_with("..."));
        assert!((text.chars().count() as i32) * 7 <= available);
        let x = center_title_x(400, text.chars().count() as i32 * 7, 22, 304, TITLE_PAD);
        assert!(x >= 22 + TITLE_PAD);
        assert!(x + (text.chars().count() as i32 * 7) <= 304 - TITLE_PAD);
        assert_eq!(elide_title("Example", 0, |_| 7), "");
        assert_eq!(elide_title("Example", 14, |_| 7), "Ex");
    }

    #[test]
    fn control_geometry_tiles_titlebar_right_edge() {
        let titlebar = SceneRect::new(100, 20, 400, 31);
        let geometries = control_button_geometries(titlebar);
        assert_eq!(
            geometries.map(|geometry| geometry.role),
            WINDOW_CHROME.controls
        );
        let width = i32::from(WINDOW_CHROME.metrics.button_width);
        assert_eq!(geometries[2].bounds.x + width, titlebar.x + titlebar.width);
        assert_eq!(geometries[1].bounds.x + width, geometries[2].bounds.x);
        assert_eq!(geometries[0].bounds.x + width, geometries[1].bounds.x);
    }

    #[test]
    fn control_slots_are_38_by_31_and_flush_to_titlebar_right_edge() {
        let geometries = control_button_geometries(SceneRect::new(0, 0, 400, 31));
        for geometry in &geometries {
            assert_eq!(geometry.bounds.width, 38);
            assert_eq!(geometry.bounds.height, 31);
        }
        assert_eq!(geometries[2].bounds.x + geometries[2].bounds.width, 400);
    }
}
