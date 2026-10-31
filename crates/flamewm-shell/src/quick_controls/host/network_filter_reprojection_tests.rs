/// Every Wi-Fi filter keystroke (typed or backspace) must request reprojection, so
/// `tick_host` projects and redraws the open popup. Source-scan over the keyboard
/// section of `route_helper_event`, which needs a live runtime to call directly.
/// Source of `route_helper_event`'s keyboard section, up to the outside-release check.
pub(super) fn keyboard_section() -> &'static str {
    include_str!("helper_events.rs")
        .split_once("pub fn route_helper_event(")
        .expect("route_helper_event must exist")
        .1
        .split_once("let open_surface = surfaces.open")
        .expect("keyboard section must precede the outside-release check")
        .0
}

#[test]
fn each_network_filter_edit_requests_reprojection() {
    let keyboard = keyboard_section();
    assert!(keyboard.contains("snapshot.network_query.filter.pop();"));
    assert!(keyboard.contains("snapshot.network_query.filter.push_str(text);"));
    assert!(
        edit_block_requests_reprojection(keyboard, "snapshot.network_query.filter.pop();"),
        "backspace filter edit must request reprojection"
    );
    assert!(
        edit_block_requests_reprojection(keyboard, "snapshot.network_query.filter.push_str(text);"),
        "typed filter edit must request reprojection"
    );
}

/// True when the branch that starts at `edit` sets `reprojection` before the
/// next `} else` (or the end of the keyboard section for the last branch).
pub(super) fn edit_block_requests_reprojection(keyboard: &str, edit: &str) -> bool {
    let rest = keyboard
        .split_once(edit)
        .expect("edit must exist in the keyboard section")
        .1;
    let block = rest.split_once("} else").map_or(rest, |(block, _)| block);
    block.contains("*reprojection = true;")
}
