//! Scroll translation and clipping. The render-core paint/layout owners keep
//! per-node scroll offsets but never move descendants or clip them, so the
//! app applies both here: after layout, descendants of every
//! `overflow-y: auto|scroll` node shift by `-offset` (hit testing follows),
//! and paint groups nodes by their scroll-viewport chain with PushClip/PopClip.

use super::*;
use flamewm_render_core::{InteractionState as Interaction, Overflow};

pub(crate) fn is_scroll_container(
    document: &RuntimeDocument,
    index: u32,
    interaction: Interaction,
) -> bool {
    matches!(
        document.runtime_style(index, interaction).overflow_y,
        Overflow::Auto | Overflow::Scroll
    )
}

/// Any element whose overflow clips its descendants to its box (`hidden`, `auto`, `scroll`).
/// Only `auto|scroll` also translate content by a scroll offset.
pub(crate) fn clips_descendants(
    document: &RuntimeDocument,
    index: u32,
    interaction: Interaction,
) -> bool {
    let style = document.runtime_style(index, interaction);
    [style.overflow_x, style.overflow_y]
        .iter()
        .any(|mode| matches!(mode, Overflow::Hidden | Overflow::Auto | Overflow::Scroll))
}

fn children(document: &RuntimeDocument, index: u32) -> impl Iterator<Item = u32> + '_ {
    std::iter::successors(document.document.nodes[index as usize].first_child, |&c| {
        document.document.nodes[c as usize].next_sibling
    })
}

/// Sets each `overflow-y: auto|scroll` container's content height from its laid-out
/// descendants (bottom edge plus bottom padding). The engine's intrinsic measure returns an
/// explicit `height` unchanged, so a scroller whose size the app sets reports no overflow and
/// gets no track or thumb. Call on a fresh layout, before `translate_scrolled_content`.
pub(crate) fn fit_scroll_extents(
    layout: &mut LayoutResult,
    document: &RuntimeDocument,
    interaction: Interaction,
) {
    for index in 0..document.document.nodes.len() as u32 {
        let style = document.runtime_style(index, interaction);
        if !matches!(style.overflow_y, Overflow::Auto | Overflow::Scroll) {
            continue;
        }
        let viewport = layout.boxes[index as usize].rect;
        let mut bottom = viewport.y + viewport.height;
        let mut pending: Vec<u32> = children(document, index).collect();
        while let Some(child) = pending.pop() {
            let rect = layout.boxes[child as usize].rect;
            if rect.width > 0.0 && rect.height > 0.0 {
                bottom = bottom.max(rect.y + rect.height);
            }
            pending.extend(children(document, child));
        }
        let fitted = bottom - viewport.y + style.padding.bottom;
        let mut content = layout.contents[index as usize];
        content.x = viewport.x;
        content.y = viewport.y;
        content.height = content.height.max(fitted).max(viewport.height);
        layout.contents[index as usize] = content;
    }
}

/// Shifts every descendant of a scroll container by the clamped offset.
/// Call once per freshly computed layout.
pub(crate) fn translate_scrolled_content(
    layout: &mut LayoutResult,
    document: &RuntimeDocument,
    interaction: Interaction,
) {
    if document.document.nodes.is_empty() {
        return;
    }
    let mut stack = vec![(document.document.root, 0.0f32, 0.0f32)];
    while let Some((index, dx, dy)) = stack.pop() {
        let slot = index as usize;
        if dx != 0.0 || dy != 0.0 {
            layout.boxes[slot].rect.x += dx;
            layout.boxes[slot].rect.y += dy;
            layout.contents[slot].x += dx;
            layout.contents[slot].y += dy;
        }
        let (mut cdx, mut cdy) = (dx, dy);
        if is_scroll_container(document, index, interaction) {
            let offset = document.scroll_offset(index);
            let m = layout.metrics_for(index, offset);
            let offset = offset.clamped(m.max_x, m.max_y);
            cdx -= offset.offset_x;
            cdy -= offset.offset_y;
        }
        stack.extend(children(document, index).map(|c| (c, cdx, cdy)));
    }
}

/// Paint commands with each scroll container's descendants clipped to its
/// viewport. `layout` must already be translated.
pub(crate) fn build_scrolled_paint(
    document: &RuntimeDocument,
    layout: &mut LayoutResult,
    interaction: Interaction,
) -> Vec<PaintCommand> {
    let paint = |layout: &LayoutResult| {
        build_paint_commands_with_scroll(document, layout, interaction, &|i| {
            document.scroll_offset(i)
        })
    };
    let mut chain: Vec<Vec<Rect>> = vec![Vec::new(); document.document.nodes.len()];
    let mut stack = vec![document.document.root];
    let mut any_clip = false;
    while let Some(index) = stack.pop() {
        let mut inherited = chain[index as usize].clone();
        if clips_descendants(document, index, interaction) {
            inherited.push(layout.boxes[index as usize].rect);
            any_clip = true;
        }
        for child in children(document, index) {
            chain[child as usize] = inherited.clone();
            stack.push(child);
        }
    }
    if !any_clip {
        return paint(layout);
    }
    let order = std::mem::take(&mut layout.z_order);
    let mut out = Vec::new();
    let mut start = 0;
    while start < order.len() {
        let clips = &chain[order[start] as usize];
        let mut end = start + 1;
        while end < order.len() && chain[order[end] as usize] == *clips {
            end += 1;
        }
        layout.z_order = order[start..end].to_vec();
        out.extend(clips.iter().map(|&rect| PaintCommand::PushClip { rect }));
        out.extend(paint(layout));
        out.extend(clips.iter().map(|_| PaintCommand::PopClip));
        start = end;
    }
    layout.z_order = order;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_render_core::{
        CompiledDocument, CompiledNode, Length, NodeKind, ScrollDelta, Style,
    };

    /// Scroll container (auto height, viewport forced to the surface) whose
    /// single child is taller than the viewport; the layout engine measures
    /// scroll extent from auto-height content, as the settings page does.
    fn fixture() -> RuntimeDocument {
        fixture_with_height(None)
    }

    fn fixture_with_height(height: Option<f32>) -> RuntimeDocument {
        let mut scroller = Style::default();
        scroller.width = Length::Px(300.0);
        scroller.overflow_y = Overflow::Auto;
        if let Some(h) = height {
            scroller.height = Length::Px(h);
        }
        let mut tall = Style::default();
        tall.width = Length::Px(180.0);
        tall.height = Length::Px(500.0);
        tall.background =
            flamewm_render_core::ColorValue::Literal(flamewm_render_core::Color::rgb(9, 9, 9));
        let node = |parent, first_child, style: Style| CompiledNode {
            kind: NodeKind::Element,
            parent,
            first_child,
            next_sibling: None,
            id: std::borrow::Cow::Borrowed(""),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
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
            nodes: vec![node(None, Some(1), scroller), node(Some(0), None, tall)],
        })
        .expect("scroll fixture is a valid document")
    }

    #[test]
    fn scrolled_descendant_moves_up_and_is_clipped_to_viewport() {
        let mut doc = fixture();
        let it = Interaction::default();
        let mut layout = LayoutEngine::compute(&doc, 300.0, 100.0, it);
        let max_y = layout.metrics_for(0, Default::default()).max_y;
        assert_eq!(max_y, 400.0);
        doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 48.0 }, (0.0, max_y));
        translate_scrolled_content(&mut layout, &doc, it);
        assert_eq!(layout.boxes[1].rect.y, -48.0);
        assert_eq!(layout.boxes[0].rect.y, 0.0, "container itself stays put");

        let commands = build_scrolled_paint(&doc, &mut layout, it);
        let viewport = layout.boxes[0].rect;
        let push = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::PushClip { rect } if *rect == viewport))
            .expect("descendants are clipped to the scroller viewport");
        let fill = commands
            .iter()
            .position(|c| matches!(c, PaintCommand::FillRect { rect, .. } if rect.y == -48.0))
            .expect("tall child painted at shifted position");
        assert!(push < fill);
        assert!(matches!(commands.last(), Some(PaintCommand::PopClip)));
    }

    #[test]
    fn explicit_height_scroller_gets_extent_from_taller_child() {
        let mut doc = fixture_with_height(Some(100.0));
        let it = Interaction::default();
        let mut layout = LayoutEngine::compute(&doc, 300.0, 100.0, it);
        assert_eq!(
            layout.metrics_for(0, Default::default()).max_y,
            0.0,
            "engine reports no overflow for an explicit height"
        );
        fit_scroll_extents(&mut layout, &doc, it);
        let max_y = layout.metrics_for(0, Default::default()).max_y;
        assert_eq!(max_y, 400.0);
        doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 1000.0 }, (0.0, max_y));
        assert_eq!(doc.scroll_offset(0).offset_y, 400.0);
    }

    #[test]
    fn offset_beyond_content_is_clamped_by_translation() {
        let mut doc = fixture();
        let it = Interaction::default();
        let mut layout = LayoutEngine::compute(&doc, 300.0, 100.0, it);
        doc.set_scroll_offset(
            0,
            flamewm_render_core::ScrollState {
                offset_x: 0.0,
                offset_y: 9999.0,
            },
            (0.0, f32::MAX),
        );
        translate_scrolled_content(&mut layout, &doc, it);
        assert_eq!(layout.boxes[1].rect.y, -400.0);
    }
}
