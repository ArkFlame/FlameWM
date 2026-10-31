//! Split from `lib.rs` (`nav`); see crate docs.

use super::*;

/// Brand block height above the first sidebar row; must match `.settings-brand` in ui/settings.css.
pub(crate) const NAV_BRAND_HEIGHT_PX: f32 = 72.0;
/// Sidebar row pitch (no gap between rows); must match `.nav-row` height in ui/settings.css.
pub(crate) const NAV_ROW_PITCH_PX: f32 = 30.0;
/// Search field block (30 px field plus 6 px bottom margin) between the brand and the rows.
pub(crate) const NAV_SEARCH_BLOCK_PX: f32 = 36.0;
/// Sidebar pages in row order, with the labels the search filters on.
pub(crate) const NAV_PAGES: [(&str, &str); 13] = [
    ("theme", "Theme"),
    ("start-menu", "Start Menu"),
    ("surfaces", "Surfaces"),
    ("animations", "Animations"),
    ("lock-screen", "Lock Screen"),
    ("screensaver", "Screensaver"),
    ("desktop", "Desktop"),
    ("taskbar", "Taskbar"),
    ("displays", "Displays"),
    ("fonts", "Fonts"),
    ("hotkeys", "Hotkeys"),
    ("assistant", "Assistant"),
    ("about", "About"),
];

/// Case-insensitive contains match; an empty query shows every page.
pub(crate) fn nav_matches(label: &str, query: &str) -> bool {
    query.is_empty() || label.to_lowercase().contains(&query.to_lowercase())
}

/// Top of the active-row indicator for sidebar index `index`, from the same layout as the CSS rows.
pub(crate) fn nav_top(index: usize) -> f32 {
    NAV_BRAND_HEIGHT_PX + NAV_SEARCH_BLOCK_PX + index as f32 * NAV_ROW_PITCH_PX
}
