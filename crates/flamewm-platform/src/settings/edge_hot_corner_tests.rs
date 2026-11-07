use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

#[test]
fn hot_corner_and_edge_snap_defaults() {
    let settings = ProductSettings::default();
    assert_eq!(settings.hot_corner_top_left, HotCornerAction::None);
    assert_eq!(settings.hot_corner_top_right, HotCornerAction::None);
    assert_eq!(settings.hot_corner_bottom_left, HotCornerAction::None);
    assert_eq!(settings.hot_corner_bottom_right, HotCornerAction::None);
    assert!(settings.edge_snap_enabled);
    assert_eq!(settings.edge_snap_threshold_px, 16);
    settings.validate().expect("defaults are valid");
}

#[test]
fn hot_corner_actions_parse_their_wire_values() {
    for (wire, action) in [
        ("none", HotCornerAction::None),
        ("overview", HotCornerAction::Overview),
        ("show-desktop", HotCornerAction::ShowDesktop),
        ("lock", HotCornerAction::Lock),
    ] {
        assert_eq!(HotCornerAction::parse(wire), Some(action));
        assert_eq!(action.as_str(), wire);
    }
    assert_eq!(HotCornerAction::parse("Lock"), None);
}

#[test]
fn hot_corners_and_edge_snap_round_trip_through_file_text() {
    let text = "revision=2\nhotCorner.topLeft=overview\nhotCorner.topRight=show-desktop\n\
                hotCorner.bottomLeft=lock\nhotCorner.bottomRight=none\nedgeSnap.enabled=0\n\
                edgeSnap.thresholdPx=40\n";
    let parsed = load(text).expect("valid file");
    assert_eq!(parsed.hot_corner_top_left, HotCornerAction::Overview);
    assert_eq!(parsed.hot_corner_top_right, HotCornerAction::ShowDesktop);
    assert_eq!(parsed.hot_corner_bottom_left, HotCornerAction::Lock);
    assert_eq!(parsed.hot_corner_bottom_right, HotCornerAction::None);
    assert!(!parsed.edge_snap_enabled);
    assert_eq!(parsed.edge_snap_threshold_px, 40);

    let serialized = ConfigStore::serialize(&parsed);
    assert_eq!(load(&serialized).expect("reload"), parsed);
}

#[test]
fn default_hot_corner_and_edge_snap_values_are_not_written() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(!text.contains("hotCorner."));
    assert!(!text.contains("edgeSnap."));
}

#[test]
fn unknown_hot_corner_action_is_rejected() {
    let error = load("revision=2\nhotCorner.topLeft=explode\n").expect_err("bad action");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn non_numeric_edge_snap_threshold_is_rejected() {
    let error = load("revision=2\nedgeSnap.thresholdPx=wide\n").expect_err("bad number");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn file_edge_snap_threshold_is_clamped_to_four_through_sixty_four() {
    let low = load("revision=2\nedgeSnap.thresholdPx=1\n").expect("clamped low");
    assert_eq!(low.edge_snap_threshold_px, 4);
    let high = load("revision=2\nedgeSnap.thresholdPx=500\n").expect("clamped high");
    assert_eq!(high.edge_snap_threshold_px, 64);
    let edge = load("revision=2\nedgeSnap.thresholdPx=64\n").expect("boundary");
    assert_eq!(edge.edge_snap_threshold_px, 64);
}

#[test]
fn validate_rejects_edge_snap_threshold_outside_range() {
    for value in [3, 65] {
        let settings = ProductSettings {
            edge_snap_threshold_px: value,
            ..ProductSettings::default()
        };
        let error = settings.validate().expect_err("out of range");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}
