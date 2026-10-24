use super::{
    CONTROL_MASK, MOD4_MASK, X11WindowRole, XK_ESCAPE, XK_LEFT, XK_SUPER_L, canonical_key_name,
    desktop_takes_focus_on_press, normalized_modifiers, printable_key_text,
};

fn reactor_loop_source() -> &'static str {
    const SOURCE: &str = include_str!("../../events.rs");
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
    let render = include_str!("../../render.rs");
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
