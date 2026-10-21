//! Desktop presentation contracts learned from the current native desktop renderer.

use flamewm_api::{Point, Rect, Size};
use flamewm_ui_core::context_menu::{
    self, MIN_ROW_WIDTH, MenuMetrics, MenuPart, permanent_delete_parts,
};
use flamewm_ui_core::popover::{PopoverEdge, PopoverGeometry};

pub const WATERMARK_WIDTH: i32 = 220;
pub const WATERMARK_HEIGHT: i32 = 73;
pub const WATERMARK_PADDING: i32 = 24;
/// Extra bottom clearance on top of the padding; keeps the CSS `bottom` value
/// (ui/desktop/desktop.css `.watermark`) at its current visual position.
pub const WATERMARK_BOTTOM_CLEARANCE: i32 = 31;
pub const DESKTOP_SELECTION_INSET: i32 = 4;
pub const DESKTOP_LABEL_RESERVE: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatermarkGeometry {
    pub rect: Rect,
}

#[must_use]
pub fn desktop_icon_candidates(preferred: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if !preferred.trim().is_empty() {
        candidates.push(preferred.trim().to_owned());
    }
    for fallback in ["text-x-generic", "file"] {
        if !candidates.iter().any(|item| item == fallback) {
            candidates.push(fallback.to_owned());
        }
    }
    candidates
}

#[must_use]
pub fn desktop_item_visual_rect(item: Rect) -> Rect {
    let inset = DESKTOP_SELECTION_INSET.max(0);
    Rect::new(
        item.x + inset,
        item.y + inset,
        (item.width - inset * 2).max(0),
        (item.height - DESKTOP_LABEL_RESERVE - inset * 2).max(0),
    )
}

/// Split a desktop icon label into at most two renderer-neutral text lines.
///
/// Defaults observed in the native desktop renderer: `width_px = 76.0`,
/// `font_size = 13.0`. Capacity per line is
/// `floor(width_px / (font_size * 0.56))` clamped to a minimum of 8 chars.
/// Counting and slicing are Unicode-safe (`chars`, never bytes). Word
/// boundaries (whitespace) are preferred; a single over-long token is hard
/// split by chars and the final line is truncated to `capacity - 1` chars
/// plus `'…'`. Centering and ellipsis styling remain the CSS/renderer job.
#[must_use]
pub fn desktop_label_lines(text: &str, width_px: f32, font_size: f32) -> [String; 2] {
    let capacity = {
        let per_char = font_size * 0.56_f32;
        if per_char > 0.0_f32 && width_px > 0.0_f32 {
            (width_px / per_char).floor() as usize
        } else {
            8_usize
        }
        .max(8_usize)
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return [String::new(), String::new()];
    }
    let char_len = |s: &str| s.chars().count();
    let take_chars = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let skip_chars = |s: &str, n: usize| s.chars().skip(n).collect::<String>();
    let truncate_final = |s: &str| {
        if char_len(s) <= capacity {
            s.to_owned()
        } else {
            let mut out = take_chars(s, capacity.saturating_sub(1));
            out.push('…');
            out
        }
    };

    if char_len(trimmed) <= capacity {
        return [trimmed.to_owned(), String::new()];
    }

    let words: Vec<&str> = trimmed.split_whitespace().collect();
    // Single unbroken token: hard split across both lines.
    if words.len() == 1 {
        let word = words[0];
        let first = take_chars(word, capacity);
        let rest = skip_chars(word, capacity);
        return [first, truncate_final(&rest)];
    }

    // Greedy word-boundary pack for the first line.
    let mut first_len = 0_usize;
    let mut first_count = 0_usize;
    for word in &words {
        let word_len = char_len(word);
        if word_len > capacity {
            break;
        }
        let needed = if first_count == 0 {
            word_len
        } else {
            first_len + 1 + word_len
        };
        if needed <= capacity {
            first_len = needed;
            first_count += 1;
        } else {
            break;
        }
    }

    if first_count == 0 {
        // Leading token exceeds capacity: hard split it, remainder goes to line two.
        let first = take_chars(words[0], capacity);
        let mut rest = skip_chars(words[0], capacity);
        if words.len() > 1 {
            rest.push(' ');
            rest.push_str(&words[1..].join(" "));
        }
        return [first, truncate_final(&rest)];
    }

    let first_line = words[..first_count].join(" ");
    let rest = words[first_count..].join(" ");
    [first_line, truncate_final(&rest)]
}

#[must_use]
pub fn watermark_geometry(
    work_area: Rect,
    width: i32,
    height: i32,
    padding: i32,
) -> WatermarkGeometry {
    let x = (work_area.right() - width - padding).max(work_area.x);
    let y = (work_area.bottom() - height - padding).max(work_area.y);
    WatermarkGeometry {
        rect: Rect::new(x, y, width.max(0), height.max(0)),
    }
}

#[must_use]
pub fn default_watermark_geometry(work_area: Rect) -> WatermarkGeometry {
    watermark_geometry(
        work_area,
        WATERMARK_WIDTH,
        WATERMARK_HEIGHT,
        WATERMARK_PADDING,
    )
}

#[must_use]
pub const fn watermark_visible(fullscreen: bool, user_enabled: bool) -> bool {
    user_enabled && !fullscreen
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlankDesktopAction {
    OpenTerminal,
    CreateNewFolder,
    NewStickyNote,
    AddVirtualDesktop,
    DesktopAndWallpaper,
}
impl BlankDesktopAction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenTerminal => "Open Terminal",
            Self::CreateNewFolder => "Create New Folder",
            Self::NewStickyNote => "New Sticky Note",
            Self::AddVirtualDesktop => "Add Virtual Desktop",
            Self::DesktopAndWallpaper => flamewm_ui_core::context_menu::SETTINGS_ROW_LABEL,
        }
    }
}

#[must_use]
pub fn blank_context_menu(sticky_enabled: bool) -> Vec<BlankDesktopAction> {
    let mut actions = vec![
        BlankDesktopAction::OpenTerminal,
        BlankDesktopAction::CreateNewFolder,
    ];
    if sticky_enabled {
        actions.push(BlankDesktopAction::NewStickyNote);
    }
    actions.push(BlankDesktopAction::AddVirtualDesktop);
    actions.push(BlankDesktopAction::DesktopAndWallpaper);
    actions
}

/// Fixed context-menu footprint used for work-area clamping.
/// Canonical shared row minimum; the outer container minimum is derived in
/// `context_menu_size` as content width plus shared container padding.
pub const CONTEXT_MENU_WIDTH: i32 = context_menu::MENU_MIN_WIDTH;
pub const CONTEXT_MENU_ROW_HEIGHT: i32 = context_menu::ROW_HEIGHT;

/// Blank-menu part shape: one row per visible action.
#[must_use]
pub fn blank_menu_parts(row_count: usize) -> Vec<MenuPart> {
    vec![MenuPart::Row; row_count]
}

/// Entry-menu part shape: one row per visible action.
#[must_use]
pub fn entry_menu_parts(row_count: usize) -> Vec<MenuPart> {
    vec![MenuPart::Row; row_count]
}

/// Sticky-menu part shape: six color/bold/delete rows (J05).
#[must_use]
pub fn sticky_menu_parts() -> Vec<MenuPart> {
    vec![MenuPart::Row; 6]
}

/// Confirm-menu part shape: shared modal header + Move to Trash/Cancel.
#[must_use]
pub fn confirm_menu_parts() -> Vec<MenuPart> {
    permanent_delete_parts()
}

/// Outer menu size from actual parts through the shared C08 contract.
/// `measured_min_width` may expand the menu; only `menu_rect` clamps.
#[must_use]
pub fn context_menu_size(parts: &[MenuPart], measured_min_width: i32) -> Size {
    context_menu::menu_size(parts, measured_min_width, MenuMetrics::canonical())
}

/// Clamp a menu anchor so the menu rect stays inside the work area.
#[must_use]
pub fn clamp_menu_anchor(work_area: Rect, anchor: Rect, menu_size: (i32, i32)) -> Rect {
    let size = Size::new(
        menu_size.0.max(0).min(work_area.width.max(0)),
        menu_size.1.max(0).min(work_area.height.max(0)),
    );
    context_menu::menu_rect(work_area, Point::new(anchor.x, anchor.y), size)
}

/// Menu height for a row count at the shared canonical row height.
#[must_use]
pub fn menu_height_for_rows(rows: usize) -> i32 {
    context_menu_size(&blank_menu_parts(rows), MIN_ROW_WIDTH).height
}

/// Canonical popover placement for a context menu anchored at a pointer.
/// The pointer is treated as a 1px source rect; the menu prefers opening
/// below/right of the press and clamps into the work area through the
/// canonical `PopoverGeometry` owner. Returns the placed rect.
#[must_use]
pub fn context_menu_rect(work_area: Rect, anchor: Point, rows: usize) -> Rect {
    let size = context_menu_size(&blank_menu_parts(rows), MIN_ROW_WIDTH);
    let source = Rect::new(anchor.x, anchor.y, 1, 1);
    let placed = PopoverGeometry::place(source, size, PopoverEdge::Below, work_area, 0);
    Rect::from_parts(placed.origin, size)
}

/// Pointer-anchored menu rect for actual parts through the shared C08
/// contract: size from `menu_size`, clamp through shared `menu_rect`.
#[must_use]
pub fn parts_menu_rect(work_area: Rect, anchor: Point, parts: &[MenuPart]) -> Rect {
    let size = context_menu_size(parts, MIN_ROW_WIDTH);
    context_menu::menu_rect(work_area, anchor, size)
}

pub const SELECTION_ACCENT_RGB: u32 = 0xff5533;
pub const SELECTION_FILL_BASE_RGB: u32 = 0x111111;
pub const SELECTION_BORDER_BASE_RGB: u32 = 0xe6e6e6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopSelectionMaterial {
    pub fill_rgb: u32,
    pub border_rgb: u32,
}

#[must_use]
pub fn selection_material(
    fill_opacity_percent: u8,
    border_opacity_percent: u8,
) -> DesktopSelectionMaterial {
    DesktopSelectionMaterial {
        fill_rgb: blend_rgb(
            SELECTION_ACCENT_RGB,
            SELECTION_FILL_BASE_RGB,
            fill_opacity_percent,
        ),
        border_rgb: blend_rgb(
            SELECTION_ACCENT_RGB,
            SELECTION_BORDER_BASE_RGB,
            border_opacity_percent,
        ),
    }
}

#[must_use]
pub fn blend_rgb(foreground: u32, background: u32, opacity_percent: u8) -> u32 {
    let foreground_weight = u32::from(opacity_percent.min(100));
    let background_weight = 100_u32 - foreground_weight;
    let blend_channel = |shift: u32| {
        let foreground_channel = (foreground >> shift) & 0xff_u32;
        let background_channel = (background >> shift) & 0xff_u32;
        ((foreground_channel * foreground_weight + background_channel * background_weight)
            / 100_u32)
            << shift
    };
    blend_channel(16_u32) | blend_channel(8_u32) | blend_channel(0_u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watermark_is_bottom_right_and_hidden_fullscreen() {
        let g = watermark_geometry(Rect::new(0, 0, 1920, 1040), 220, 73, 24);
        assert_eq!(g.rect, Rect::new(1676, 943, 220, 73));
        assert!(!watermark_visible(true, true));
    }
    #[test]
    fn sticky_menu_item_is_conditional() {
        assert_eq!(blank_context_menu(false).len(), 4);
        assert_eq!(
            blank_context_menu(true)[2],
            BlankDesktopAction::NewStickyNote
        );
        let without = blank_context_menu(false);
        assert!(without.contains(&BlankDesktopAction::AddVirtualDesktop));
        assert_eq!(
            without.last(),
            Some(&BlankDesktopAction::DesktopAndWallpaper)
        );
    }
    #[test]
    fn menu_anchor_clamps_inside_work_area() {
        let work_area = Rect::new(0, 0, 1920, 1040);
        let clamped = clamp_menu_anchor(work_area, Rect::new(1900, 1020, 205, 165), (205, 165));
        assert_eq!(clamped, Rect::new(1715, 875, 205, 165));
        assert_eq!(menu_height_for_rows(4), 134);
    }
    #[test]
    fn menu_rows_map_to_fixed_row_geometry() {
        assert_eq!(CONTEXT_MENU_WIDTH, 205);
        assert_eq!(CONTEXT_MENU_ROW_HEIGHT, 31);
        assert_eq!(menu_height_for_rows(1), 41);
        assert_eq!(menu_height_for_rows(2), 72);
        assert_eq!(menu_height_for_rows(5), 165);
    }
    #[test]
    fn blank_menu_row_visibility_follows_sticky_flag() {
        // 4 rows without sticky, 5 with: hidden rows stay display:none.
        assert_eq!(menu_height_for_rows(blank_context_menu(false).len()), 134);
        assert_eq!(menu_height_for_rows(blank_context_menu(true).len()), 165);
        assert!(!blank_context_menu(false).contains(&BlankDesktopAction::NewStickyNote));
        assert!(blank_context_menu(true).contains(&BlankDesktopAction::NewStickyNote));
    }
    #[test]
    fn menu_anchor_clamps_zero_sized_menu_inside_work_area() {
        let work_area = Rect::new(0, 0, 1920, 1040);
        let clamped = clamp_menu_anchor(work_area, Rect::new(100, 100, 205, 0), (205, 0));
        assert_eq!(clamped, Rect::new(100, 100, 205, 0));
    }
    #[test]
    fn material_blend_matches_native_integer_blend() {
        assert_eq!(blend_rgb(0xff5533, 0x111111, 20), 0x401e17);
    }
    #[test]
    fn zero_opacity_returns_background() {
        assert_eq!(blend_rgb(0xff5533, 0x111111, 0), 0x111111);
    }
    #[test]
    fn full_opacity_returns_foreground() {
        assert_eq!(blend_rgb(0xff5533, 0x111111, 100), 0xff5533);
    }
    #[test]
    fn default_selection_material_matches_native_colors_and_opacity() {
        let m = selection_material(20, 60);
        assert_eq!(m.fill_rgb, 0x401e17);
        assert_eq!(m.border_rgb, blend_rgb(0xff5533, 0xe6e6e6, 60));
    }
    #[test]
    fn default_watermark_uses_current_native_size() {
        assert_eq!(
            default_watermark_geometry(Rect::new(0, 0, 1920, 1040)).rect,
            Rect::new(1676, 943, 220, 73)
        );
    }
    #[test]
    fn desktop_css_accent_states_use_live_tokens_only() {
        // Menu hover lives in the shared menu stylesheet; the desktop surface and menus together
        // must read accent states from live tokens only.
        let desktop = include_str!("../../../ui/desktop/desktop.css");
        let menu = include_str!("../../../ui/shared/menu.css");
        let css = format!("{desktop}\n{menu}");
        for literal in ["ef4048", "EF4048", "239, 64, 72", "239,64,72"] {
            assert!(
                !css.contains(literal),
                "desktop.css/menu.css hard-code accent {literal}"
            );
        }
        for token in [
            "var(--desktop-hover)",
            "var(--desktop-hover-border)",
            "var(--selection-fill)",
            "var(--accent-strong)",
            "var(--accent-soft)",
            "var(--accent)",
        ] {
            assert!(css.contains(token), "desktop.css/menu.css lack {token}");
        }
    }

    #[test]
    fn watermark_css_offsets_match_core_padding_constants() {
        let css = include_str!("../../../ui/desktop/desktop.css");
        let rule_start = css.find(".watermark {").expect("missing .watermark rule");
        let body = &css[rule_start..rule_start + css[rule_start..].find('}').expect("unclosed")];
        let px = |prop: &str| -> i32 {
            let key = format!("{prop}:");
            let start = body.find(&key).unwrap_or_else(|| panic!("missing {prop}")) + key.len();
            body[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("integer px")
        };
        assert_eq!(px("right"), WATERMARK_PADDING);
        assert_eq!(px("bottom"), WATERMARK_PADDING + WATERMARK_BOTTOM_CLEARANCE);
    }

    #[test]
    fn desktop_icon_fallback_keeps_semantic_then_generic_order() {
        assert_eq!(
            desktop_icon_candidates("folder"),
            vec!["folder", "text-x-generic", "file"]
        );
    }
    #[test]
    fn desktop_selection_material_excludes_label_area() {
        assert_eq!(
            desktop_item_visual_rect(Rect::new(10, 10, 80, 96)),
            Rect::new(14, 14, 72, 72)
        );
    }
    #[test]
    fn label_splits_backup_downloads_on_word_boundary() {
        assert_eq!(
            desktop_label_lines("BACKUP DOWNLOADS", 76.0, 13.0),
            ["BACKUP".to_owned(), "DOWNLOADS".to_owned()]
        );
    }
    #[test]
    fn label_keeps_single_short_name_on_first_line() {
        assert_eq!(
            desktop_label_lines("Docs", 76.0, 13.0),
            ["Docs".to_owned(), String::new()]
        );
    }
    #[test]
    fn label_truncates_long_unbroken_token_with_ellipsis() {
        let [first, second] = desktop_label_lines("Superlongfilenamewithoutspaces", 76.0, 13.0);
        assert_eq!(first, "Superlongf");
        assert_eq!(second, "ilenamewi…");
        assert!(second.chars().count() <= 10);
        assert!(second.ends_with('…'));
    }
    #[test]
    fn label_counts_unicode_chars_not_bytes() {
        let [first, second] = desktop_label_lines("café résumé naïve", 76.0, 13.0);
        assert_eq!(first, "café");
        assert_eq!(second, "résumé na…");
        assert!(second.chars().count() <= 10);
    }
    #[test]
    fn label_bounds_single_long_word_lines_to_capacity() {
        // Capacity at 76px / 13px font is floor(76 / 7.28) = 10 chars per line.
        for text in ["readme.txt", "a-very-long-filename.txt"] {
            let [first, second] = desktop_label_lines(text, 76.0, 13.0);
            assert!(
                first.chars().count() <= 10,
                "first line too long: {first:?}"
            );
            assert!(
                second.chars().count() <= 10,
                "second line too long: {second:?}"
            );
        }
        let [first, second] = desktop_label_lines("a-very-long-filename.txt", 76.0, 13.0);
        assert_eq!(first, "a-very-lon");
        assert_eq!(second, "g-filenam…");
    }
    #[test]
    fn label_empty_returns_two_empty_lines() {
        assert_eq!(
            desktop_label_lines("", 76.0, 13.0),
            [String::new(), String::new()]
        );
        assert_eq!(
            desktop_label_lines("   ", 76.0, 13.0),
            [String::new(), String::new()]
        );
    }

    #[test]
    fn desktop_settings_row_uses_shared_flamewm_settings_label() {
        assert_eq!(
            BlankDesktopAction::DesktopAndWallpaper.label(),
            flamewm_ui_core::context_menu::SETTINGS_ROW_LABEL
        );
    }
}
