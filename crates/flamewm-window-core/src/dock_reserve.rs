//! Work-area space reserved by the Dock.
//!
//! The taskbar reserves its own strip (layer-shell zone, `_NET_WM_STRUT`, or platform panel) before
//! this runs. Applying the Dock as a further cut from the same edge stacks the two without any
//! special case: a bottom Dock above a bottom taskbar cuts 44 px, then `dock_extent` px more.

use flamewm_api::Rect;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Screen edge a Dock reserves space on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Bar padding on each side, in px. Mirrors `PAD` in `crates/flamewm-shell/src/dock.rs`.
pub const DOCK_PADDING: i32 = 6;
/// Gap around each icon, in px. Mirrors `GAP` in `crates/flamewm-shell/src/dock.rs`.
pub const DOCK_GAP: i32 = 3;
/// Gap between the bar and its screen edge. Mirrors `DOCK_MARGIN` in `crates/flamewm-shell/src/dock.rs`.
pub const DOCK_MARGIN: i32 = 8;
/// Default `dock.iconSize` (the settings default).
pub const DEFAULT_ICON_SIZE: i32 = 32;

/// Thickness the Dock reserves on its edge: icon plus gap and padding on both sides, plus the
/// floating margin. The hover label room is an overlay and is not reserved.
#[must_use]
pub fn dock_extent(icon_size: i32) -> i32 {
    icon_size + 2 * (DOCK_PADDING + DOCK_GAP) + DOCK_MARGIN
}

/// Cuts `extent` px off `work` along `edge`. Sizes never go negative.
#[must_use]
pub fn reserve_edge(work: Rect, edge: ReserveEdge, extent: i32) -> Rect {
    let extent = extent.max(0);
    match edge {
        ReserveEdge::Top => Rect::new(
            work.x,
            work.y.saturating_add(extent),
            work.width,
            (work.height - extent).max(0),
        ),
        ReserveEdge::Bottom => Rect::new(work.x, work.y, work.width, (work.height - extent).max(0)),
        ReserveEdge::Left => Rect::new(
            work.x.saturating_add(extent),
            work.y,
            (work.width - extent).max(0),
            work.height,
        ),
        ReserveEdge::Right => Rect::new(work.x, work.y, (work.width - extent).max(0), work.height),
    }
}

/// `(edge, extent)` the Dock reserves, or `None` when `dock.enabled` is not true, when
/// `dock.reserveSpace` is not true (default off: windows may overlap the dock), or when
/// `dock.autoHide` is on (an auto-hidden Dock never reserves; it only overlaps while revealed).
///
/// Missing or ill-typed values keep the settings defaults (`autoHide` off, `reserveSpace` off). An
/// unknown `dock.position` falls back to bottom, and an out-of-range `dock.iconSize` falls back to
/// the default, so a hand-edited file cannot move the reserve off the screen.
#[must_use]
pub fn dock_reserve_from_snapshot(snapshot: &SettingsSnapshot) -> Option<(ReserveEdge, i32)> {
    if !matches!(
        snapshot.values.get("dock.enabled"),
        Some(SettingValue::Boolean(true))
    ) {
        return None;
    }
    if !matches!(
        snapshot.values.get("dock.reserveSpace"),
        Some(SettingValue::Boolean(true))
    ) {
        return None;
    }
    if matches!(
        snapshot.values.get("dock.autoHide"),
        Some(SettingValue::Boolean(true))
    ) {
        return None;
    }
    let edge = match snapshot.values.get("dock.position") {
        Some(SettingValue::Text(position)) => match position.as_str() {
            "top" => ReserveEdge::Top,
            "left" => ReserveEdge::Left,
            "right" => ReserveEdge::Right,
            _ => ReserveEdge::Bottom,
        },
        _ => ReserveEdge::Bottom,
    };
    let icon_size = match snapshot.values.get("dock.iconSize") {
        Some(SettingValue::Integer(size)) if (24..=96).contains(size) => *size as i32,
        _ => DEFAULT_ICON_SIZE,
    };
    Some((edge, dock_extent(icon_size)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const OUTPUT: Rect = Rect::new(0, 0, 1280, 800);

    fn snapshot(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let values: BTreeMap<_, _> = entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect();
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values = values;
        snapshot
    }

    #[test]
    fn each_edge_cuts_only_its_own_side() {
        assert_eq!(
            reserve_edge(OUTPUT, ReserveEdge::Bottom, 72),
            Rect::new(0, 0, 1280, 728)
        );
        assert_eq!(
            reserve_edge(OUTPUT, ReserveEdge::Top, 72),
            Rect::new(0, 72, 1280, 728)
        );
        assert_eq!(
            reserve_edge(OUTPUT, ReserveEdge::Left, 72),
            Rect::new(72, 0, 1208, 800)
        );
        assert_eq!(
            reserve_edge(OUTPUT, ReserveEdge::Right, 72),
            Rect::new(0, 0, 1208, 800)
        );
    }

    #[test]
    fn dock_above_taskbar_on_the_same_edge_stacks() {
        let after_taskbar = reserve_edge(OUTPUT, ReserveEdge::Bottom, 44);
        let after_dock = reserve_edge(after_taskbar, ReserveEdge::Bottom, dock_extent(48));
        assert_eq!(
            after_dock,
            Rect::new(0, 0, 1280, 800 - 44 - dock_extent(48))
        );
    }

    #[test]
    fn dock_and_taskbar_on_different_edges_do_not_stack() {
        let after_taskbar = reserve_edge(OUTPUT, ReserveEdge::Top, 44);
        let after_dock = reserve_edge(after_taskbar, ReserveEdge::Bottom, 72);
        assert_eq!(after_dock, Rect::new(0, 44, 1280, 800 - 44 - 72));
    }

    #[test]
    fn zero_and_oversized_reserves_never_go_negative() {
        assert_eq!(reserve_edge(OUTPUT, ReserveEdge::Bottom, 0), OUTPUT);
        assert_eq!(reserve_edge(OUTPUT, ReserveEdge::Left, -5), OUTPUT);
        let squeezed = reserve_edge(OUTPUT, ReserveEdge::Bottom, 9_000);
        assert_eq!(squeezed.height, 0);
    }

    #[test]
    fn extent_matches_the_shell_bar_thickness() {
        assert_eq!(dock_extent(48), 74);
        assert_eq!(dock_extent(96), 122);
    }

    fn reserved(position: &str, extra: &[(&str, SettingValue)]) -> Option<(ReserveEdge, i32)> {
        let mut entries = vec![
            ("dock.enabled", SettingValue::Boolean(true)),
            ("dock.reserveSpace", SettingValue::Boolean(true)),
            ("dock.position", SettingValue::Text(position.into())),
        ];
        entries.extend(extra.iter().cloned());
        dock_reserve_from_snapshot(&snapshot(&entries))
    }

    #[test]
    fn disabled_or_missing_dock_reserves_nothing() {
        assert_eq!(dock_reserve_from_snapshot(&snapshot(&[])), None);
        assert_eq!(
            dock_reserve_from_snapshot(&snapshot(&[
                ("dock.enabled", SettingValue::Boolean(false)),
                ("dock.reserveSpace", SettingValue::Boolean(true)),
                ("dock.position", SettingValue::Text("left".into())),
            ])),
            None
        );
    }

    #[test]
    fn reserve_space_defaults_off_so_windows_may_overlap_the_dock() {
        assert_eq!(
            dock_reserve_from_snapshot(&snapshot(&[
                ("dock.enabled", SettingValue::Boolean(true)),
                ("dock.position", SettingValue::Text("bottom".into())),
            ])),
            None,
            "a missing dock.reserveSpace means the default (off)"
        );
        assert_eq!(
            dock_reserve_from_snapshot(&snapshot(&[
                ("dock.enabled", SettingValue::Boolean(true)),
                ("dock.reserveSpace", SettingValue::Boolean(false)),
                ("dock.autoHide", SettingValue::Boolean(false)),
            ])),
            None
        );
    }

    #[test]
    fn enabled_dock_reads_position_and_icon_size() {
        assert_eq!(
            reserved(
                "right",
                &[
                    ("dock.autoHide", SettingValue::Boolean(false)),
                    ("dock.iconSize", SettingValue::Integer(64)),
                ]
            ),
            Some((ReserveEdge::Right, dock_extent(64)))
        );
    }

    #[test]
    fn invalid_position_and_icon_size_fall_back_to_defaults() {
        assert_eq!(
            reserved(
                "sideways",
                &[
                    ("dock.autoHide", SettingValue::Boolean(false)),
                    ("dock.iconSize", SettingValue::Integer(500)),
                ]
            ),
            Some((ReserveEdge::Bottom, dock_extent(DEFAULT_ICON_SIZE)))
        );
    }

    #[test]
    fn default_icon_size_is_32() {
        assert_eq!(DEFAULT_ICON_SIZE, 32);
        assert_eq!(dock_extent(DEFAULT_ICON_SIZE), 32 + 2 * (6 + 3) + 8);
    }

    /// The four edges with their reserve side, for per-edge rules.
    const EDGES: [(&str, ReserveEdge); 4] = [
        ("bottom", ReserveEdge::Bottom),
        ("top", ReserveEdge::Top),
        ("left", ReserveEdge::Left),
        ("right", ReserveEdge::Right),
    ];

    #[test]
    fn auto_hide_on_reserves_nothing_on_every_edge() {
        for (position, _) in EDGES {
            assert_eq!(
                reserved(position, &[("dock.autoHide", SettingValue::Boolean(true))]),
                None,
                "auto-hide on, {position}"
            );
        }
    }

    #[test]
    fn auto_hide_off_with_reserve_reserves_the_extent_on_every_edge() {
        for (position, edge) in EDGES {
            assert_eq!(
                reserved(position, &[("dock.autoHide", SettingValue::Boolean(false))]),
                Some((edge, dock_extent(DEFAULT_ICON_SIZE))),
                "auto-hide off, {position}"
            );
        }
    }

    #[test]
    fn missing_auto_hide_is_off_so_a_reserved_dock_reserves() {
        assert_eq!(
            reserved("bottom", &[]),
            Some((ReserveEdge::Bottom, dock_extent(DEFAULT_ICON_SIZE))),
            "a missing dock.autoHide means the default (off)"
        );
        assert_eq!(
            dock_reserve_from_snapshot(&snapshot(&[
                ("dock.enabled", SettingValue::Boolean(true)),
                ("dock.reserveSpace", SettingValue::Boolean(true)),
                ("dock.autoHide", SettingValue::Boolean(true)),
            ])),
            None
        );
    }
}

/// Cuts the Dock's reserve from a work area. A hidden Dock (`None`) leaves the work area unchanged.
/// Both backends apply the Dock as the last cut, after the taskbar or strut reserve.
#[must_use]
pub fn cut_dock(work: Rect, dock: Option<(ReserveEdge, i32)>) -> Rect {
    match dock {
        Some((edge, extent)) => reserve_edge(work, edge, extent),
        None => work,
    }
}

#[cfg(test)]
mod cut_dock_vector_tests {
    use super::*;

    /// Vectors shared with the X11 `with_dock_reserve` and Wayland `cut_dock` pins.
    #[test]
    fn dock_stacks_above_a_bottom_taskbar() {
        let work = Rect::new(0, 0, 1280, 800 - 44);
        assert_eq!(
            cut_dock(work, Some((ReserveEdge::Bottom, dock_extent(48)))),
            Rect::new(0, 0, 1280, 800 - 44 - dock_extent(48))
        );
        assert_eq!(cut_dock(work, None), work);
    }
}
