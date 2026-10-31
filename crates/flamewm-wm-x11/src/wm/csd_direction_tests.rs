use super::*;

#[test]
fn moveresize_directions_map_clockwise_from_top_left() {
    // EWMH _NET_WM_MOVERESIZE: 0 TL, 1 T, 2 TR, 3 R, 4 BR, 5 B, 6 BL, 7 L.
    assert_eq!(resize_edges_for_direction(0), ResizeEdges::top_left());
    assert_eq!(resize_edges_for_direction(1), ResizeEdges::top());
    assert_eq!(resize_edges_for_direction(2), ResizeEdges::top_right());
    assert_eq!(resize_edges_for_direction(3), ResizeEdges::right());
    assert_eq!(resize_edges_for_direction(4), ResizeEdges::bottom_right());
    assert_eq!(resize_edges_for_direction(5), ResizeEdges::bottom());
    assert_eq!(resize_edges_for_direction(6), ResizeEdges::bottom_left());
    assert_eq!(resize_edges_for_direction(7), ResizeEdges::left());
}

#[test]
fn moveresize_edge_mapping_rejects_plain_move() {
    // Negative control: direction 8 (move) must not be a resize edge.
    assert_eq!(resize_edges_for_direction(8), ResizeEdges::left());
}

#[test]
fn click_on_inactive_client_requests_focus_and_active_client_does_not() {
    let client: Window = 0x0040_0002;
    assert!(super::should_focus_on_press(None, client));
    assert!(super::should_focus_on_press(Some(0x0040_0001), client));
    // Negative control: a plain click on the already-active window must not refocus/restack.
    assert!(!super::should_focus_on_press(Some(client), client));
}

#[test]
fn client_press_reasserts_focus_on_active_client_and_skips_wheel() {
    // Regression: a click on the cached-active client used to skip X input focus entirely,
    // so a server-side focus divergence stayed until the window was moved (title press focuses).
    let client: Window = 0x0040_0002;
    assert_eq!(press_focus_action(None, client, 1), PressFocus::Focus);
    assert_eq!(press_focus_action(Some(0x0040_0001), client, 1), PressFocus::Focus);
    assert_eq!(press_focus_action(Some(client), client, 1), PressFocus::Reassert);
    assert_eq!(press_focus_action(Some(client), client, 3), PressFocus::Reassert);
    // Wheel buttons (4/5) are never focus requests.
    assert_eq!(press_focus_action(Some(client), client, 4), PressFocus::Skip);
    assert_eq!(press_focus_action(None, client, 5), PressFocus::Skip);
}
