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
/// Separator line thickness (`.menu-separator { height:1px }`).
pub const SEPARATOR_LINE: i32 = 1;
/// Separator margin above and below the line (`.menu-separator { margin:5px }`).
pub const SEPARATOR_MARGIN: i32 = 5;
/// Separator vertical footprint: line plus top and bottom margin.
pub const SEPARATOR_HEIGHT: i32 = SEPARATOR_LINE + 2 * SEPARATOR_MARGIN;
/// Outer container minimum width: row minimum plus both container paddings.
pub const MENU_MIN_WIDTH: i32 = MIN_ROW_WIDTH + 2 * MENU_PADDING;
/// Corner radius of a shell `.menu-row` in px.
pub const ROW_RADIUS: i32 = 3;
/// Row icon edge length in px.
pub const ROW_ICON_SIZE: i32 = 16;
/// Gap between row icon and label in px.
pub const ROW_GAP: i32 = 9;
/// Left inset of row content in px.
pub const ROW_PADDING_LEFT: i32 = 8;
/// CSS custom property that fills a hovered row.
pub const ROW_HOVER_TOKEN: &str = "--accent-soft";

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

// Legacy context-menu imports remain valid; all materials now have one shared owner.
pub use crate::surface::{SurfaceFill, blend_surface_over_rgb, surface_fill, surface_stroke};

/// Shared row model for desktop and taskbar context menus (single owner: this module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRowKind {
    Action,
    Header,
    Separator,
}

/// One context-menu row. `icon` is a freedesktop icon name, resolved by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuRow {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: Option<&'static str>,
    pub enabled: bool,
    pub visible: bool,
    pub kind: MenuRowKind,
}

pub const SETTINGS_ROW_ID: &str = "flamewm-settings";
pub const SETTINGS_ROW_LABEL: &str = "FlameWM Settings";
pub const SETTINGS_ROW_ICON: &str = "preferences-desktop";

/// The "`FlameWM` Settings" row, identical on the desktop and taskbar menus.
#[must_use]
pub const fn settings_row(id: &'static str) -> MenuRow {
    MenuRow {
        id,
        label: SETTINGS_ROW_LABEL,
        icon: Some(SETTINGS_ROW_ICON),
        enabled: true,
        visible: true,
        kind: MenuRowKind::Action,
    }
}

/// Container surface for every context-menu root: corner radius and fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuSurface {
    pub radius: i32,
    pub fill: SurfaceFill,
}

/// Projected surface for a menu root from the Surfaces settings (`decoration`, `surfaceColor`, `surfaceOpacity`).
#[must_use]
pub fn menu_surface(snapshot: &SettingsSnapshot) -> MenuSurface {
    let squared = matches!(
        snapshot.values.get("decoration"),
        Some(SettingValue::Text(value)) if value == "squared"
    );
    MenuSurface {
        radius: shape_radius(!squared),
        fill: surface_fill(snapshot),
    }
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

    #[test]
    fn menu_clamps_inside_the_output_containing_the_anchor_near_shared_edge() {
        fn inside(outer: Rect, inner: Rect) -> bool {
            inner.x >= outer.x
                && inner.y >= outer.y
                && inner.right() <= outer.right()
                && inner.bottom() <= outer.bottom()
        }
        let size = Size::new(205, 160);
        // Left 1920x1080 at origin, right 1280x720 offset down by 180; shared edge at x=1920.
        let left = Rect::new(0, 0, 1920, 1080);
        let right = Rect::new(1920, 180, 1280, 720);
        // Anchor 5px left of the shared edge stays on the left output and flips inside it.
        let on_left = menu_rect(left, Point::new(1915, 300), size);
        assert_eq!(on_left, Rect::new(1715, 300, 205, 160));
        assert!(inside(left, on_left));
        // Anchor 5px right of the shared edge on the right output stays inside the right output.
        let on_right = menu_rect(right, Point::new(1925, 800), size);
        assert_eq!(on_right, Rect::new(1925, 740, 205, 160));
        assert!(inside(right, on_right));
        // Negative-origin left output at x=-1280: anchor at its right edge flips inside it, not into x=0.
        let negative_left = Rect::new(-1280, 0, 1280, 720);
        let flipped = menu_rect(negative_left, Point::new(-1, 710), size);
        assert_eq!(flipped, Rect::new(0 - 205, 720 - 160, 205, 160));
        assert!(inside(negative_left, flipped));
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
                rgb: 0x0019_1b1d,
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
                rgb: 0x001e_2123,
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
        assert_eq!(surface_fill(&bare).rgb, 0x00ef_4048);
        let invalid = snapshot_with("surfaceColor", SettingValue::Text("#zzzzzz".into()));
        assert_eq!(surface_fill(&invalid).rgb, 0x0019_1b1d);
    }

    #[test]
    fn surface_fill_alpha_is_independent_of_colour_value() {
        let snapshot = snapshot_with("surfaceOpacity", SettingValue::Percent(50));
        let fill = surface_fill(&snapshot);
        assert_eq!(fill.alpha, 127);
        assert_eq!(fill.rgb, 0x0019_1b1d);
    }
}

/// Drift guard: the shell and desktop stylesheets must express the canonical
/// metrics for the properties they already set. Properties not yet expressed
/// are migrated by the consumer wave, not asserted here.
#[cfg(test)]
mod css_contract_tests {
    use super::*;

    const SHELL_CSS: &str = include_str!("../../../ui/shell/flamewm.css");
    const DESKTOP_CSS: &str = include_str!("../../../ui/desktop/desktop.css");
    const MENU_CSS: &str = include_str!("../../../ui/shared/menu.css");
    const SHELL_BUILD: &str = include_str!("../../flamewm-shell/build.rs");
    const DESKTOP_BUILD: &str = include_str!("../../flamewm-desktop/build.rs");

    /// Declaration block of the first top-level rule whose selector list contains `selector`.
    fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let mut rest = css;
        while let Some(open) = rest.find('{') {
            let close = rest[open..].find('}').map_or(rest.len(), |i| open + i);
            if rest[..open].split(',').any(|item| item.trim() == selector) {
                return &rest[open + 1..close];
            }
            rest = &rest[(close + 1).min(rest.len())..];
        }
        panic!("rule `{selector}` not found");
    }

    fn decl<'a>(css: &'a str, selector: &str, property: &str) -> &'a str {
        rule(css, selector)
            .split(';')
            .find_map(|declaration| {
                let (name, value) = declaration.split_once(':')?;
                (name.trim() == property).then(|| value.trim())
            })
            .unwrap_or_else(|| panic!("`{selector}` has no `{property}`"))
    }

    fn px(css: &str, selector: &str, property: &str) -> i32 {
        let raw = decl(css, selector, property);
        raw.strip_suffix("px")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("`{selector} {property}` is not px: {raw}"))
    }

    /// Every selector item in `css`, with comments removed.
    fn selectors(css: &str) -> Vec<String> {
        let mut plain = String::new();
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            plain.push_str(&rest[..start]);
            rest = rest[start..]
                .find("*/")
                .map_or("", |end| &rest[start + end + 2..]);
        }
        plain.push_str(rest);
        let blocks = plain.matches('{').count();
        plain
            .split('{')
            .take(blocks)
            .flat_map(|head| {
                head.rsplit('}')
                    .next()
                    .unwrap_or("")
                    .split(',')
                    .map(|item| item.trim().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn shared_menu_expresses_canonical_metrics_once() {
        let css = MENU_CSS;
        assert_eq!(px(css, ".context-menu", "min-width"), MENU_MIN_WIDTH);
        assert_eq!(px(css, ".context-menu", "padding"), MENU_PADDING);
        assert_eq!(
            px(css, ".context-menu", "border-radius"),
            shape_radius(true)
        );
        assert_eq!(px(css, ".menu-row", "min-width"), MIN_ROW_WIDTH);
        assert_eq!(px(css, ".menu-row", "height"), ROW_HEIGHT);
        assert_eq!(px(css, ".menu-row", "padding-left"), ROW_PADDING_LEFT);
        assert_eq!(px(css, ".menu-row", "gap"), ROW_GAP);
        assert_eq!(px(css, ".menu-row", "border-radius"), ROW_RADIUS);
        assert_eq!(px(css, ".menu-row img", "width"), ROW_ICON_SIZE);
        assert_eq!(px(css, ".menu-row img", "height"), ROW_ICON_SIZE);
        assert_eq!(px(css, ".menu-header", "min-width"), MIN_ROW_WIDTH);
        assert_eq!(px(css, ".menu-header", "min-height"), ROW_HEIGHT);
        assert_eq!(
            px(css, ".menu-separator", "height") + 2 * px(css, ".menu-separator", "margin"),
            SEPARATOR_HEIGHT
        );
        let hover = format!("var({ROW_HOVER_TOKEN})");
        assert_eq!(decl(css, ".menu-row:hover", "background"), hover.as_str());
    }

    #[test]
    fn menu_rules_are_owned_only_by_shared_sheet() {
        for owned in [
            ".context-menu",
            ".menu-row",
            ".menu-header",
            ".menu-separator",
        ] {
            for (name, css) in [("flamewm.css", SHELL_CSS), ("desktop.css", DESKTOP_CSS)] {
                assert!(
                    !selectors(css).iter().any(|item| item == owned),
                    "{name} must not define `{owned}`; it lives in ui/shared/menu.css"
                );
            }
            assert!(selectors(MENU_CSS).iter().any(|item| item == owned));
        }
    }

    #[test]
    fn both_compiled_documents_link_the_shared_menu_sheet() {
        assert!(SHELL_BUILD.contains("ui/shared/menu.css"));
        assert!(DESKTOP_BUILD.contains("ui/shared/menu.css"));
    }

    #[test]
    fn accent_soft_hover_token_is_identical_across_sheets() {
        assert_eq!(
            decl(SHELL_CSS, ":root", ROW_HOVER_TOKEN),
            decl(DESKTOP_CSS, ":root", ROW_HOVER_TOKEN)
        );
    }
}

#[cfg(test)]
mod shared_row_tests {
    use super::*;

    #[test]
    fn settings_row_is_the_single_flamewm_settings_contract() {
        let row = settings_row(SETTINGS_ROW_ID);
        assert_eq!(row.label, "FlameWM Settings");
        assert_eq!(row.icon, Some("preferences-desktop"));
        assert_eq!(row.kind, MenuRowKind::Action);
        assert!(row.enabled);
        assert!(row.visible);
    }

    #[test]
    fn menu_surface_radius_follows_decoration_setting() {
        let rounded = menu_surface(&SettingsSnapshot::new(1));
        assert_eq!(rounded.radius, 12);
        assert_eq!(
            rounded.fill,
            SurfaceFill {
                rgb: 0x0019_1b1d,
                alpha: 127
            }
        );
        let mut squared = SettingsSnapshot::new(1);
        squared
            .values
            .insert("decoration".into(), SettingValue::Text("squared".into()));
        assert_eq!(menu_surface(&squared).radius, 0);
    }
}
