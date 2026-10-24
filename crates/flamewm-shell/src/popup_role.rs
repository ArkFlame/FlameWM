//! Popup role -> node identity mapping for the shell.
//!
//! Canonical owner of the semantic mapping between popup roles, intrinsic
//! (popup document root) node ids, panel source (anchor) node ids, semantic
//! names, and helper [`QuickControlKind`] conversion.
//!
//! Source evidence:
//! - Parent shell `runtime.rs`: `open_start_group_measured` anchors Start to
//!   the retained `task-start` panel node; `open_status_anchored_id` anchors
//!   status popups to `status_source_id(name)` (`tray-media`, `tray-volume`,
//!   `tray-network`, `clock-button`).
//! - Quick helper `quick_controls/host.rs`: `open_request` owns one popup
//!   surface per [`QuickControlKind`].
//! - Transport `quick_controls/protocol.rs`: [`QuickControlKind`]
//!   (`Audio`, `Network`, `Calendar`) is transport-only; this module imports
//!   it for conversion only and adds no transport behavior.
//! - Panel nodes: `tray-media`, `tray-volume`, `tray-network`, `clock-button`;
//!   start anchor `task-start`.
//! - Semantic roots: `start-menu`, `media-popup`, `audio-popup`,
//!   `wifi-popup`, `clock-popup` (RAW HTML ids, NEVER `'#...'` oracle).
//! - There is no Display/Brightness helper popup (Settings Displays only, no
//!   Brightness symbol); no such role is defined here.

use crate::quick_controls::QuickControlKind;

/// Canonical popup role owned by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupRole {
    Start,
    Media,
    Audio,
    Network,
    Calendar,
    Battery,
    Weather,
    /// Top-center application launcher. Never anchored to a panel node: `source_node_id`
    /// is inert for it and placement comes from `runtime::launcher_surface`.
    Launcher,
}

impl PopupRole {
    /// Intrinsic (popup document root) node id.
    /// RAW HTML id, NEVER '#...'.
    #[must_use]
    pub const fn intrinsic_node_id(self) -> &'static str {
        match self {
            Self::Start => "start-menu",
            Self::Media => "media-popup",
            Self::Audio => "audio-popup",
            Self::Network => "wifi-popup",
            Self::Calendar => "clock-popup",
            Self::Battery => "battery-popup",
            Self::Weather => "weather-popup",
            Self::Launcher => "launcher-popup",
        }
    }

    /// Panel source (anchor) node id.
    #[must_use]
    pub const fn source_node_id(self) -> &'static str {
        match self {
            Self::Start => "task-start",
            Self::Media => "tray-media",
            Self::Audio => "tray-volume",
            Self::Network => "tray-network",
            Self::Calendar => "clock-button",
            Self::Battery => "tray-battery",
            Self::Weather => "tray-weather",
            Self::Launcher => "task-start",
        }
    }

    /// Canonical semantic name. `Calendar` maps to `"calendar"` (protocol
    /// `QuickControlKind::as_str`); `"clock"` remains a legacy alias handled
    /// at call sites (`runtime::open_status`, `main::open_quick_control`)
    /// and is not a separate role.
    #[must_use]
    pub const fn semantic_name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Media => "media",
            Self::Audio => "audio",
            Self::Network => "network",
            Self::Calendar => "calendar",
            Self::Battery => "battery",
            Self::Weather => "weather",
            Self::Launcher => "launcher",
        }
    }

    /// Helper-process conversion. Start and Media open in-process (`None`);
    /// Audio, Network, and Calendar delegate to the quick-control helper
    /// (`Some`).
    #[must_use]
    pub const fn quick_control_kind(self) -> Option<QuickControlKind> {
        match self {
            Self::Start | Self::Media | Self::Launcher => None,
            Self::Audio => Some(QuickControlKind::Audio),
            Self::Network => Some(QuickControlKind::Network),
            Self::Calendar => Some(QuickControlKind::Calendar),
            Self::Battery => Some(QuickControlKind::Battery),
            Self::Weather => Some(QuickControlKind::Weather),
        }
    }
}

#[cfg(test)]
mod popup_contract_tests {
    use super::*;
    use flamewm_api::{PanelEdge, Rect, Size};
    use flamewm_shell_core::popup::{
        fitted_start_placement, measured_popup_rect, work_area_for_panel,
    };
    use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};

    fn artifact_bytes(role: PopupRole) -> &'static [u8] {
        match role {
            PopupRole::Start => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-start.rwr")),
            PopupRole::Media => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-media.rwr")),
            PopupRole::Audio => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr")),
            PopupRole::Network => {
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr"))
            }
            PopupRole::Calendar => {
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-calendar.rwr"))
            }
            PopupRole::Battery => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr")),
            PopupRole::Weather => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-weather.rwr")),
            PopupRole::Launcher => include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-launcher.rwr")),
        }
    }

    /// Real CSS intrinsic for each role. Start is `.start-menu`
    /// (`width:550px`) by the tallest child box, `.start-content-pane`
    /// (`min-height:420px`): the pre-A12-3 `292x380` was the left-pane width
    /// and a stale height, so the surface was mapped ~250px too narrow.
    fn expected_intrinsic(role: PopupRole) -> Size {
        match role {
            PopupRole::Start => Size::new(550, 420),
            PopupRole::Media => Size::new(310, 200),
            PopupRole::Audio => Size::new(310, 220),
            PopupRole::Network => Size::new(310, 260),
            PopupRole::Calendar => Size::new(300, 320),
            PopupRole::Battery => flamewm_shell_core::BATTERY_POPOVER_SIZE,
            PopupRole::Weather => flamewm_shell_core::WEATHER_POPOVER_SIZE,
            PopupRole::Launcher => Size::new(600, 360),
        }
    }

    // CONTRACT-REGRESSION: popup intrinsic node ids must resolve in the real compiled artifacts.
    // TRIGGER: PopupRole::intrinsic_node_id changes or artifact template id renames.
    // OBSERVABLE: decode of the role artifact + node lookup by intrinsic_node_id succeeds.
    // GAP: F02/F03 — '#' -prefixed ids never match compiled document ids, so lookup fails.
    // MUTATION: strip/add '#' prefix on any intrinsic_node_id; test must flip.
    // CASE: T01 cross-contract RED — decode real artifact, resolve node, pin intrinsic bounds.
    #[test]
    fn t01_popup_intrinsic_node_resolves_in_real_artifact() {
        let work_area = Rect::new(0, 0, 1350, 597);
        for role in [
            PopupRole::Start,
            PopupRole::Media,
            PopupRole::Audio,
            PopupRole::Network,
            PopupRole::Calendar,
            PopupRole::Battery,
            PopupRole::Weather,
            PopupRole::Launcher,
        ] {
            let mut document = flamewm_ui_x11::decode_document(artifact_bytes(role))
                .unwrap_or_else(|error| panic!("{role:?} artifact must decode: {error}"));
            // Exercises RuntimeDocument::node_by_id internally: missing id
            // returns Err, so this MUST FAIL while ids carry '#' prefixes.
            document
                .visible(role.intrinsic_node_id(), true)
                .unwrap_or_else(|error| {
                    panic!(
                        "{role:?} node '{}' must resolve: {error}",
                        role.intrinsic_node_id()
                    )
                });
            let intrinsic = expected_intrinsic(role);
            // i32 extents are finite by type; the RED contract is node
            // resolution above plus positive, in-work-area bounds here.
            assert!(
                intrinsic.width > 0 && intrinsic.height > 0,
                "{role:?} intrinsic must be positive"
            );
            assert!(
                intrinsic.width < work_area.width && intrinsic.height < work_area.height,
                "{role:?} intrinsic must fit inside the work area"
            );
        }
    }

    // CONTRACT-REGRESSION: bottom-panel popup placement matrix at 1350x641.
    // TRIGGER: anchor math or placement API changes in flamewm-shell-core::popup.
    // OBSERVABLE: every popup rect sits above panel top (y=597), right-side popups stay right of midpoint.
    // GAP: F04 anchor math is correct but unpinned; regressions would overlap panel or span work area.
    // MUTATION: move any rect bottom past 597 or left of midpoint; test must fail.
    // CASE: T02 pure placement matrix — Start fitted + status popups measured, no full-area rects.
    #[test]
    fn t02_popup_placement_matrix_bottom_panel() {
        let output = Rect::new(0, 0, 1350, 641);
        let panel = Rect::new(0, 597, 1350, 44);
        let work_area = work_area_for_panel(output, panel, PanelEdge::Bottom);
        assert_eq!(work_area, Rect::new(0, 0, 1350, 597));
        let midpoint = work_area.x + work_area.width / 2;

        let start_anchor = Rect::new(0, 597, 43, 44);
        let start = fitted_start_placement(
            start_anchor,
            expected_intrinsic(PopupRole::Start),
            PanelEdge::Bottom,
            work_area,
            8,
        )
        .expect("start placement");
        assert!(start.rect.bottom() <= 597, "start {:?}", start.rect);
        // A12-3: at 1350x641 the real CSS intrinsic (550x420) fits, so Start
        // must open at it uncapped. A stale 292x380 fails this pin.
        let start_intrinsic = expected_intrinsic(PopupRole::Start);
        assert_eq!(
            (start.rect.width, start.rect.height),
            (start_intrinsic.width, start_intrinsic.height),
            "start must open at its real CSS intrinsic size: {:?}",
            start.rect
        );
        assert!(!start.content_was_capped, "start fits at 1350x641");

        let slots = [
            (PopupRole::Media, Rect::new(1350 - 220, 597, 30, 44)),
            (PopupRole::Audio, Rect::new(1350 - 180, 597, 30, 44)),
            (PopupRole::Network, Rect::new(1350 - 140, 597, 30, 44)),
            (PopupRole::Calendar, Rect::new(1350 - 100, 597, 30, 44)),
        ];
        let mut rects = vec![start.rect];
        for (role, anchor) in slots {
            let rect = measured_popup_rect(
                Some(anchor),
                Some(expected_intrinsic(role)),
                PopoverEdge::Above,
                PopoverAlign::OutputEnd,
                work_area,
                8,
            )
            .unwrap_or_else(|error| panic!("{role:?} placement must succeed: {error}"));
            assert!(rect.bottom() <= 597, "{role:?} {rect:?}");
            assert!(rect.x > midpoint, "{role:?} {rect:?}");
            assert_eq!(rect.right(), output.right(), "{role:?} {rect:?}");
            rects.push(rect);
        }
        for rect in &rects {
            assert_ne!(
                *rect, work_area,
                "{rect:?} must not equal the full work area"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrinsic_node_ids() {
        assert_eq!(PopupRole::Start.intrinsic_node_id(), "start-menu");
        assert_eq!(PopupRole::Media.intrinsic_node_id(), "media-popup");
        assert_eq!(PopupRole::Audio.intrinsic_node_id(), "audio-popup");
        assert_eq!(PopupRole::Network.intrinsic_node_id(), "wifi-popup");
        assert_eq!(PopupRole::Calendar.intrinsic_node_id(), "clock-popup");
        assert_eq!(PopupRole::Battery.intrinsic_node_id(), "battery-popup");
        assert_eq!(PopupRole::Weather.intrinsic_node_id(), "weather-popup");
        assert_eq!(PopupRole::Weather.source_node_id(), "tray-weather");
        assert_eq!(PopupRole::Weather.semantic_name(), "weather");
    }

    #[test]
    fn battery_role_is_sized_and_anchored_to_the_tray_battery_node() {
        assert_eq!(
            flamewm_shell_core::BATTERY_POPOVER_SIZE,
            flamewm_api::Size::new(300, 132)
        );
        assert_eq!(PopupRole::Battery.source_node_id(), "tray-battery");
        assert_eq!(PopupRole::Battery.semantic_name(), "battery");
    }

    #[test]
    fn source_node_ids() {
        assert_eq!(PopupRole::Start.source_node_id(), "task-start");
        assert_eq!(PopupRole::Media.source_node_id(), "tray-media");
        assert_eq!(PopupRole::Audio.source_node_id(), "tray-volume");
        assert_eq!(PopupRole::Network.source_node_id(), "tray-network");
        assert_eq!(PopupRole::Calendar.source_node_id(), "clock-button");
        assert_eq!(PopupRole::Battery.source_node_id(), "tray-battery");
    }

    #[test]
    fn semantic_names() {
        assert_eq!(PopupRole::Start.semantic_name(), "start");
        assert_eq!(PopupRole::Media.semantic_name(), "media");
        assert_eq!(PopupRole::Audio.semantic_name(), "audio");
        assert_eq!(PopupRole::Network.semantic_name(), "network");
        assert_eq!(PopupRole::Calendar.semantic_name(), "calendar");
        assert_eq!(PopupRole::Battery.semantic_name(), "battery");
    }

    #[test]
    fn quick_control_kind_conversion() {
        assert_eq!(PopupRole::Start.quick_control_kind(), None);
        assert_eq!(PopupRole::Media.quick_control_kind(), None);
        assert_eq!(
            PopupRole::Audio.quick_control_kind(),
            Some(QuickControlKind::Audio)
        );
        assert_eq!(
            PopupRole::Network.quick_control_kind(),
            Some(QuickControlKind::Network)
        );
        assert_eq!(
            PopupRole::Calendar.quick_control_kind(),
            Some(QuickControlKind::Calendar)
        );
        assert_eq!(
            PopupRole::Battery.quick_control_kind(),
            Some(QuickControlKind::Battery)
        );
        assert_eq!(
            PopupRole::Weather.quick_control_kind(),
            Some(QuickControlKind::Weather)
        );
    }
}
