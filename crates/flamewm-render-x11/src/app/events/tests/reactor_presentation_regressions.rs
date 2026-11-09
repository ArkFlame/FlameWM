use super::{
    CONTROL_MASK, MOD4_MASK, X11WindowRole, XK_ESCAPE, XK_LEFT, XK_SUPER_L, canonical_key_name,
    desktop_takes_focus_on_press, normalized_modifiers, printable_key_text,
};

fn reactor_loop_source() -> &'static str {
    const SOURCE: &str = include_str!("../window.rs");
    let start = SOURCE
        .find("pub(crate) unsafe fn event_loop_with_reactor")
        .expect("reactor event-loop owner remains present");
    let end = SOURCE[start..]
        .find("pub(crate) fn hit_test")
        .map(|offset| start + offset)
        .expect("reactor event-loop owner has a bounded body");
    &SOURCE[start..end]
}

#[test]
fn t18_reactor_only_document_mutation_presents_once_the_x_queue_drains() {
    let source = reactor_loop_source();
    let callback = source
        .find("on_reactor(document)?")
        .expect("reactor callback is the document mutation boundary");
    let after_callback = &source[callback..];
    let queue_gate = after_callback
        .find("XEventsQueued")
        .expect("X11 queue gate remains after reactor dispatch");
    let present = after_callback
        .find("redraw_if_dirty(document)?")
        .expect("reactor-only mutation must reach the retained redraw gate");
    assert!(
        queue_gate < present,
        "reactor document mutation is presented on the empty-queue branch, so queued input shares one frame"
    );
}

#[test]
fn t19_noop_reactor_wakes_use_the_dirty_skip_not_an_unconditional_paint() {
    let source = reactor_loop_source();
    assert_eq!(
        source.matches("redraw_if_dirty(document)?").count(),
        1,
        "one retained redraw gate must admit each reactor turn"
    );
    assert!(
        !source.contains("self.redraw("),
        "reactor wake must not become an unconditional full repaint loop"
    );
    let render = include_str!("../../render/frame.rs");
    assert!(render.contains("if self.pending_damage.is_empty() && !stale"));
    assert!(render.contains("return Ok(false);"));
}

#[test]
fn t20_same_turn_reactor_mutations_coalesce_at_one_final_state_gate() {
    let source = reactor_loop_source();
    assert_eq!(
        source.matches("on_reactor(document)?").count(),
        1,
        "one reactor callback owns the same-turn mutation batch"
    );
    assert_eq!(
        source.matches("redraw_if_dirty(document)?").count(),
        1,
        "the mutation batch must have one retained presentation gate"
    );
    let callback = source
        .find("on_reactor(document)?")
        .expect("reactor callback is present");
    let present = source[callback..]
        .find("redraw_if_dirty(document)?")
        .expect("same-turn mutations must present through the retained gate");
    assert!(present > 0);
}

#[test]
fn desktop_primary_press_claims_focus_for_selected_file_shortcuts() {
    assert!(desktop_takes_focus_on_press(X11WindowRole::Desktop, 1));
    assert!(desktop_takes_focus_on_press(X11WindowRole::Desktop, 3));
    assert!(!desktop_takes_focus_on_press(X11WindowRole::Desktop, 4));
    assert!(!desktop_takes_focus_on_press(X11WindowRole::Normal, 1));
    assert!(!desktop_takes_focus_on_press(X11WindowRole::Dock, 1));
}

#[test]
fn typed_key_projection_uses_shortcut_modifier_order_and_canonical_names() {
    assert_eq!(
        normalized_modifiers(CONTROL_MASK | MOD4_MASK).collect::<Vec<_>>(),
        vec!["Ctrl", "Super"]
    );
    assert_eq!(canonical_key_name(XK_LEFT), "Left");
    assert_eq!(canonical_key_name(XK_SUPER_L), "Super_L");
    assert_eq!(printable_key_text(u64::from(b'A')), Some("A".to_string()));
    assert_eq!(printable_key_text(XK_ESCAPE), None);
}

#[test]
fn reactor_never_blocks_with_latched_damage_but_still_honours_the_backdrop_deadline() {
    use super::reactor_wait_timeout;
    use std::time::Duration;
    let wait = Some(Duration::from_millis(30));
    assert_eq!(reactor_wait_timeout(false, false, None), None);
    assert_eq!(reactor_wait_timeout(false, false, wait), wait);
    assert_eq!(
        reactor_wait_timeout(false, true, None),
        Some(Duration::ZERO)
    );
    assert_eq!(
        reactor_wait_timeout(true, false, wait),
        Some(Duration::ZERO)
    );
    assert_eq!(
        reactor_wait_timeout(false, true, wait),
        Some(Duration::ZERO)
    );
}

#[test]
fn t21_requested_window_backdrop_is_applied_in_the_reactor_turn_before_the_paint_gate() {
    let source = reactor_loop_source();
    let callback = source
        .find("on_reactor(document)?")
        .expect("reactor callback is the document mutation boundary");
    let apply = source[callback..]
        .find("apply_window_backdrop_request(document)")
        .expect("a backdrop requested by the reactor callback is applied in the same turn");
    let gate = source[callback..]
        .find("redraw_if_dirty(document)?")
        .expect("reactor-only mutation must reach the retained redraw gate");
    assert!(
        apply < gate,
        "a requested backdrop must be installed before the paint gate, so it repaints once with no extra wake"
    );
}

#[test]
fn t21_settled_resize_is_delivered_on_the_reactor_turn_that_drains_the_queue() {
    // An early-return event (foreign window, backdrop, mapping) can drain the X queue without
    // reaching the event loop's own flush. The reactor must flush the settled size itself, and must
    // not block on the fd while that size is still pending, or the resize waits for unrelated input.
    let source = reactor_loop_source();
    let flush = source
        .find("self.flush_pending_window_resize(document, on_action)?")
        .expect("reactor turn delivers the settled window size");
    let gate = source
        .find("redraw_if_dirty(document)?")
        .expect("reactor turn keeps the retained redraw gate");
    assert!(
        flush < gate,
        "the settled size must reach the explorer before the paint that shows it"
    );
    assert!(
        source.contains("!self.pending_damage.is_empty() || self.pending_window_resize.is_some()"),
        "a pending settled size must keep the reactor wait at zero timeout"
    );
    let events = include_str!("../../events.rs");
    assert!(
        !events.contains("if self.pending_window_resize.is_some()"),
        "the flush lives in one helper, not duplicated inline"
    );
}
