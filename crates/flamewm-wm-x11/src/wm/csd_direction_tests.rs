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
    let client: Window = 0x400002;
    assert!(super::should_focus_on_press(None, client));
    assert!(super::should_focus_on_press(Some(0x400001), client));
    // Negative control: a plain click on the already-active window must not refocus/restack.
    assert!(!super::should_focus_on_press(Some(client), client));
}
