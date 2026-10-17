//! Native input helpers: PointerButton mapping (local duplicate, no
//! feature logic), wheel Button4/5 -> scroll delta, slider thumb/track
//! geometry for range drag.

use super::scroll::is_scroll_container;
use super::*;
use flamewm_render_core::ScrollDelta;

/// Local raw-X-button mapping. Mirrors `PointerButton::from_raw_x` without
/// importing feature logic; wheel buttons never act as press/release.
pub(crate) fn pointer_button_from_raw(button: u32) -> PointerButton {
    PointerButton::from_raw_x(button)
}

/// Wheel Button4/5 -> vertical scroll delta (Up = -1 line, Down = +1).
/// Returns None for non-wheel buttons.
pub(crate) fn wheel_scroll_delta(button: PointerButton) -> Option<(f32, f32)> {
    const LINE: f32 = 24.0;
    match button {
        PointerButton::WheelUp => Some((0.0, -LINE)),
        PointerButton::WheelDown => Some((0.0, LINE)),
        _ => None,
    }
}

/// Topmost node under the pointer whose overflow-y is user-scrollable
/// (Auto/Scroll). Walks the cached z-order in reverse like hit testing, but
/// does not require an action, so plain scroll containers are targets.
fn wheel_scroll_target(
    layout: &LayoutResult,
    document: &RuntimeDocument,
    interaction: InteractionState,
    x: f32,
    y: f32,
) -> Option<u32> {
    layout.z_order.iter().rev().copied().find(|&index| {
        layout
            .boxes
            .get(index as usize)
            .is_some_and(|b| b.rect.contains(x, y))
            && document.is_effectively_visible(index)
            && is_scroll_container(document, index, interaction)
    })
}

/// Applies one wheel step to the scrollable node under the pointer, clamped
/// by that node's layout metrics. Returns the scrolled node index, or None
/// when the pointer is not over a scrollable node.
pub(crate) fn apply_wheel_scroll(
    layout: &LayoutResult,
    document: &mut RuntimeDocument,
    interaction: InteractionState,
    x: f32,
    y: f32,
    delta: (f32, f32),
) -> Option<u32> {
    let index = wheel_scroll_target(layout, document, interaction, x, y)?;
    let metrics = layout.metrics_for(index, document.scroll_offset(index));
    document.apply_scroll_delta(
        index,
        ScrollDelta {
            dx: delta.0,
            dy: delta.1,
        },
        (metrics.max_x, metrics.max_y),
    );
    Some(index)
}

/// Map a pointer x on the track to a snapped value in [min, max].
/// `step <= 0` means continuous.
pub(crate) fn slider_value_from_pointer(
    track: Rect,
    min: f32,
    max: f32,
    step: f32,
    pointer_x: f32,
) -> f32 {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    let thumb_w = 12.0f32.min(track.width.max(1.0));
    let travel = (track.width - thumb_w).max(0.0);
    if travel <= 0.0 {
        return lo;
    }
    let ratio = ((pointer_x - track.x - thumb_w / 2.0) / travel).clamp(0.0, 1.0);
    let raw = lo + ratio * (hi - lo);
    if step <= 0.0 {
        raw
    } else {
        let steps = ((raw - lo) / step).round();
        (lo + steps * step).clamp(lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_render_core::{
        CompiledDocument, CompiledNode, Length, NodeKind, Overflow, ScrollState, Style,
    };

    #[test]
    fn wheel_maps_to_scroll_delta_only() {
        assert_eq!(
            wheel_scroll_delta(pointer_button_from_raw(4)),
            Some((0.0, -24.0))
        );
        assert_eq!(
            wheel_scroll_delta(pointer_button_from_raw(5)),
            Some((0.0, 24.0))
        );
        assert_eq!(wheel_scroll_delta(pointer_button_from_raw(1)), None);
    }

    /// Root is the only node whose layout viewport is forced to the surface
    /// size, so it is the overflow case the layout engine can express.
    fn wheel_fixture() -> RuntimeDocument {
        let mut root = Style::default();
        root.width = Length::Px(300.0);
        root.overflow_y = Overflow::Auto;
        let mut content = Style::default();
        content.width = Length::Px(180.0);
        content.height = Length::Px(500.0);
        let node = |parent: Option<u32>, first_child: Option<u32>, style: Style| CompiledNode {
            kind: NodeKind::Element,
            parent,
            first_child,
            next_sibling: None,
            id: String::new(),
            action: String::new(),
            text: String::new(),
            image: None,
            style,
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![node(None, Some(1), root), node(Some(0), None, content)],
        })
        .expect("wheel fixture is a valid document")
    }

    #[test]
    fn wheel_down_scrolls_overflow_auto_node_under_pointer_and_clamps_at_max() {
        let mut doc = wheel_fixture();
        let layout = LayoutEngine::compute(&doc, 300.0, 100.0, InteractionState::default());
        let max_y = layout.metrics_for(0, ScrollState::default()).max_y;
        assert_eq!(max_y, 400.0);
        let (_, down) = wheel_scroll_delta(pointer_button_from_raw(5)).expect("Button5 wheel");
        let (_, up) = wheel_scroll_delta(pointer_button_from_raw(4)).expect("Button4 wheel");

        // Pointer over the non-scrollable 180x500 child: target is the scrollable root.
        let wheel = |doc: &mut RuntimeDocument, dy: f32| {
            apply_wheel_scroll(&layout, doc, InteractionState::default(), 10.0, 10.0, (0.0, dy))
        };
        assert_eq!(wheel(&mut doc, down), Some(0));
        assert_eq!(doc.scroll_offset(0).offset_y, 24.0);

        for _ in 0..40 {
            wheel(&mut doc, down);
        }
        assert_eq!(doc.scroll_offset(0).offset_y, max_y);

        for _ in 0..40 {
            wheel(&mut doc, up);
        }
        assert_eq!(doc.scroll_offset(0).offset_y, 0.0);
    }

    #[test]
    fn wheel_outside_scrollable_node_leaves_offsets_untouched() {
        let mut doc = wheel_fixture();
        let layout = LayoutEngine::compute(&doc, 300.0, 100.0, InteractionState::default());
        let (_, down) = wheel_scroll_delta(pointer_button_from_raw(5)).expect("Button5 wheel");

        let target = apply_wheel_scroll(
            &layout,
            &mut doc,
            InteractionState::default(),
            350.0,
            50.0,
            (0.0, down),
        );
        assert_eq!(target, None);
        assert_eq!(doc.scroll_offset(0), ScrollState::default());
        assert_eq!(doc.scroll_offset(1), ScrollState::default());
    }

    #[test]
    fn slider_pointer_round_trips() {
        let track = Rect {
            x: 10.0,
            y: 0.0,
            width: 100.0,
            height: 8.0,
        };
        let v = slider_value_from_pointer(track, 0.0, 100.0, 0.0, 60.0);
        assert!((v - 50.0).abs() < 1.0);
    }
}
