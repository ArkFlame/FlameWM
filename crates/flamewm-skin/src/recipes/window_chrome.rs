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
    // Shared rim `#ffffff3f` (CSS `--rim`, 25%) composited over PANEL, so Wayland matches the X11 rim.
    border: Rgb(0x0053_5657),
    close_hover: Rgb(0x00e8_1123),
};

/// Vertical padding inside the titlebar reserved above/below the icon slot.
pub const ICON_SLOT_PAD_Y: u16 = 4;
/// Minimum icon slot edge when the titlebar is short.
pub const ICON_SLOT_MIN: u16 = 8;
/// Left padding before the icon slot.
pub const ICON_PAD_LEFT: i32 = 13;
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

/// Available title text width after respecting the left icon and right control strip.
/// Both native backends must shorten the title *before* computing its centered origin;
/// `center_title_x` alone cannot prevent overlap when the title exceeds this space.
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
