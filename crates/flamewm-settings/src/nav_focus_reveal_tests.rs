use super::view;

// REGRESSION: the sidebar's last row sits below the fold of `.nav-scroll`. Tab onto it must scroll
// the nav so the row (and its focus ring) lands inside the nav viewport, on the shipped document.
#[test]
fn tab_to_clipped_sidebar_row_scrolls_nav_into_view() {
    let document = view::document().expect("shipped settings document");
    let extent = (
        view::SETTINGS_DEFAULT_WIDTH as f32,
        view::SETTINGS_DEFAULT_HEIGHT as f32,
    );
    let probe = flamewm_ui_x11::tab_to_action_for_test(document, extent, "settings.page.about", 64)
        .expect("Tab reaches the About row");
    assert!(
        probe.offset_y > 0.0,
        "About starts clipped, so Tab must scroll .nav-scroll (offset {})",
        probe.offset_y
    );
    let [x, y, width, height] = probe.row;
    let [vx, vy, vw, vh] = probe.viewport;
    assert!(
        y >= vy && y + height <= vy + vh,
        "About row {y}..{} not inside nav viewport {vy}..{}",
        y + height,
        vy + vh
    );
    assert!(
        x >= vx && x + width <= vx + vw,
        "About row not horizontally inside nav viewport"
    );
}
