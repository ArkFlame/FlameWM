//! Shared context-menu geometry contract (C08).
//!
//! Single owner for menu sizing/placement math. Desktop and shell call
//! sites migrate to this in a later step; this file defines only the
//! shared contract and does not change existing call sites.
//!
//! Canonical base (shell canonical, skin `recipes::menu::CONTEXT_MENU`):
//! 31px row height, 5px container padding. The 195-vs-205 minimum-width
//! range winner is **195**: the measured existing `.menu-row` content
//! width (`flamewm.css` `.menu-row { width:195px; height:31px }`). The
//! outer container minimum (205) is derived as `195 + 2 * padding`, so
//! both numbers stay consistent instead of being duplicated constants.
//!
//! Permanent-delete shape (shared CSS contract): header
//! `"Delete permanently?"`, rows `Delete` / `Cancel`.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::{Point, Rect, Size};

/// Minimum `.menu-row` content width in px (measured existing value).
pub const MIN_ROW_WIDTH: i32 = 195;
/// Canonical row height in px (shell canonical).
pub const ROW_HEIGHT: i32 = 31;
/// Canonical container padding in px (shell canonical).
pub const MENU_PADDING: i32 = 5;
/// Separator vertical footprint: 1px line + 2x5px margin (matches
/// `.menu-separator { height:1px; margin:5px }`).
pub const SEPARATOR_HEIGHT: i32 = 11;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuMetrics {
    pub row_height: i32,
    pub padding: i32,
    pub separator_height: i32,
}

impl MenuMetrics {
    #[must_use]
    pub const fn canonical() -> Self {
        Self {
            row_height: ROW_HEIGHT,
            padding: MENU_PADDING,
            separator_height: SEPARATOR_HEIGHT,
        }
    }
}

/// Structural menu parts. Height is derived from actual parts: `Header`
/// and `Row` each cost one row height, `Separator` costs one separator
/// height, plus top/bottom container padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuPart {
    Header,
    Row,
    Separator,
}

/// Permanent-delete menu shape: header + two rows.
#[must_use]
pub fn permanent_delete_parts() -> Vec<MenuPart> {
    vec![MenuPart::Header, MenuPart::Row, MenuPart::Row]
}

/// Outer menu size. `measured_min_width` is the measured content width
/// (may expand the menu); it never shrinks below the 195px row minimum.
/// Only `menu_rect` clamps to the work area/output.
#[must_use]
pub fn menu_size(parts: &[MenuPart], measured_min_width: i32, metrics: MenuMetrics) -> Size {
    let mut rows: i32 = 0;
    let mut separators: i32 = 0;
    for part in parts {
        match part {
            MenuPart::Header | MenuPart::Row => rows += 1,
            MenuPart::Separator => separators += 1,
        }
    }
    let content = measured_min_width.max(MIN_ROW_WIDTH).max(0);
    let width = content + 2 * metrics.padding.max(0);
    let height = 2 * metrics.padding.max(0)
        + rows * metrics.row_height.max(0)
        + separators * metrics.separator_height.max(0);
    Size::new(width.max(1), height.max(1))
}

/// Corner radius in px for the `decoration` shape: `rounded` is 12, `squared` is 0. Geometry
/// (row height, padding, anchor clamp) is independent of the shape.
#[must_use]
pub const fn shape_radius(rounded: bool) -> i32 {
    if rounded { 12 } else { 0 }
}

/// Shared Surfaces fill for desktop menus: `surfaceColor` RGB with the `surfaceOpacity` alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceFill {
    pub rgb: u32,
    pub alpha: u8,
}

const DEFAULT_SURFACE_COLOR: u32 = 0x191b1d;
const DEFAULT_SURFACE_OPACITY: u8 = 50;

/// Maps `surfaceColor` / `surfaceOpacity` to a fill. Same mapping as the shell
/// `settings_projection::surface_background`; invalid colours fall back to the default.
#[must_use]
pub fn surface_fill(snapshot: &SettingsSnapshot) -> SurfaceFill {
    let rgb = match snapshot.values.get("surfaceColor") {
        Some(SettingValue::Text(value)) => parse_rgb(value),
        _ => None,
    }
    .unwrap_or(DEFAULT_SURFACE_COLOR);
    let percent = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => DEFAULT_SURFACE_OPACITY,
    };
    SurfaceFill {
        rgb,
        alpha: ((u16::from(percent) * 255) / 100) as u8,
    }
}

/// Parses `#rrggbb` or bare `rrggbb` into 0xRRGGBB.
fn parse_rgb(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Pointer-anchored menu rect clamped inside the work area/output.
#[must_use]
pub fn menu_rect(work_area: Rect, anchor: Point, size: Size) -> Rect {
    Rect::new(anchor.x, anchor.y, size.width, size.height).clamp_inside(work_area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_radius_follows_decoration() {
        assert_eq!(shape_radius(true), 12);
        assert_eq!(shape_radius(false), 0);
    }

    #[test]
    fn shape_does_not_change_row_geometry_or_anchor_clamp() {
        let parts = permanent_delete_parts();
        let size = menu_size(&parts, 0, MenuMetrics::canonical());
        assert_eq!(size, Size::new(205, 5 * 2 + 3 * 31));
        let work = Rect::new(0, 0, 400, 300);
        let rect = menu_rect(work, Point::new(390, 290), size);
        assert_eq!((rect.x, rect.y), (400 - size.width, 300 - size.height));
    }

    fn snapshot_with(key: &str, value: SettingValue) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(key.to_owned(), value);
        snapshot
    }

    #[test]
    fn surface_fill_defaults_match_shell_surface_mapping() {
        let fill = surface_fill(&SettingsSnapshot::new(1));
        assert_eq!(
            fill,
            SurfaceFill {
                rgb: 0x191b1d,
                alpha: 127
            }
        );
    }

    #[test]
    fn surface_fill_maps_colour_and_opacity_to_alpha() {
        let mut snapshot = snapshot_with("surfaceColor", SettingValue::Text("#1E2123".into()));
        snapshot
            .values
            .insert("surfaceOpacity".into(), SettingValue::Percent(100));
        assert_eq!(
            surface_fill(&snapshot),
            SurfaceFill {
                rgb: 0x1e2123,
                alpha: 255
            }
        );
        snapshot
            .values
            .insert("surfaceOpacity".into(), SettingValue::Percent(0));
        assert_eq!(surface_fill(&snapshot).alpha, 0);
    }

    #[test]
    fn surface_fill_accepts_bare_hex_and_rejects_invalid_colour() {
        let bare = snapshot_with("surfaceColor", SettingValue::Text("ef4048".into()));
        assert_eq!(surface_fill(&bare).rgb, 0xef4048);
        let invalid = snapshot_with("surfaceColor", SettingValue::Text("#zzzzzz".into()));
        assert_eq!(surface_fill(&invalid).rgb, 0x191b1d);
    }

    #[test]
    fn surface_fill_alpha_is_independent_of_colour_value() {
        let snapshot = snapshot_with("surfaceOpacity", SettingValue::Percent(50));
        let fill = surface_fill(&snapshot);
        assert_eq!(fill.alpha, 127);
        assert_eq!(fill.rgb, 0x191b1d);
    }
}
