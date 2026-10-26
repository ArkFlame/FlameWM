//! Scroll translation and clipping. The render-core paint/layout owners keep
//! per-node scroll offsets but never move descendants or clip them, so the
//! app applies both here: after layout, descendants of every
//! `overflow-y: auto|scroll` node shift by `-offset` (hit testing follows),
//! and paint groups nodes by their scroll-viewport chain with PushClip/PopClip.

use super::*;
use flamewm_render_core::{
    Color, InteractionState as Interaction, NodeKind, Overflow, ScrollState, Style,
};

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
    // One work stack reused across scrollers; each walk drains it back to empty.
    let mut pending: Vec<u32> = Vec::new();
    for index in 0..document.document.nodes.len() as u32 {
        let style = document.runtime_style(index, interaction);
        if !matches!(style.overflow_y, Overflow::Auto | Overflow::Scroll) {
            continue;
        }
        let viewport = layout.boxes[index as usize].rect;
        let mut bottom = viewport.y + viewport.height;
        pending.extend(children(document, index));
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
    let mut stack = Vec::with_capacity(document.document.nodes.len());
    stack.push((document.document.root, 0.0f32, 0.0f32));
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

/// Clip rectangles inherited by each node from its scroll/clip ancestors, and whether any
/// node clips at all.
/// One link of a clip chain: a clipping ancestor's rectangle plus a pointer to the chain of the
/// clipping ancestors above it. Chain 0 is the empty root (depth 0).
struct ClipChain {
    parent: u32,
    rect: Rect,
    depth: u32,
}

/// Clip chains as shared ids. `ids[node]` indexes `chains`, the chain of clipping ancestors
/// that `node` inherits. A chain is created only where a clipping node's children begin, so
/// unclipped subtrees share their parent's chain. Chains link to their parent by index, so a
/// clipping node costs one link rather than a copy of its whole ancestor rectangle list.
fn clip_chains(
    document: &RuntimeDocument,
    layout: &LayoutResult,
    interaction: Interaction,
) -> (Vec<u32>, Vec<ClipChain>, bool) {
    let mut ids = vec![0u32; document.document.nodes.len()];
    let mut chains = vec![ClipChain {
        parent: 0,
        rect: Rect::default(),
        depth: 0,
    }];
    // A hidden subtree paints nothing, so its overflow never clips anything: it keeps its parent's
    // chain and its styles are not resolved (most nodes of a pooled document are hidden).
    let mut stack = vec![(document.document.root, false)];
    let mut any_clip = false;
    while let Some((index, hidden_ancestor)) = stack.pop() {
        let mut id = ids[index as usize];
        let hidden = hidden_ancestor || !document.is_visible(index);
        if !hidden && clips_descendants(document, index, interaction) {
            let depth = chains[id as usize].depth + 1;
            chains.push(ClipChain {
                parent: id,
                rect: layout.boxes[index as usize].rect,
                depth,
            });
            id = (chains.len() - 1) as u32;
            any_clip = true;
        }
        for child in children(document, index) {
            ids[child as usize] = id;
            stack.push((child, hidden));
        }
    }
    (ids, chains, any_clip)
}

/// Walks the paint z-order in runs of equal clip chains (compared by content, so run
/// boundaries match a per-node comparison). `paint_run` appends the commands of one contiguous
/// run of z-order nodes; each run is wrapped in its PushClip/PopClip pairs.
fn build_clipped(
    document: &RuntimeDocument,
    layout: &mut LayoutResult,
    interaction: Interaction,
    mut paint_run: impl FnMut(&mut LayoutResult, &[u32], &mut Vec<PaintCommand>),
) -> Vec<PaintCommand> {
    let (ids, chains, any_clip) = clip_chains(document, layout, interaction);
    let order = std::mem::take(&mut layout.z_order);
    let mut out = Vec::with_capacity(order.len());
    if !any_clip {
        paint_run(layout, &order, &mut out);
    } else {
        let same = |a: u32, b: u32| a == b || same_chain(&chains, a, b);
        let mut start = 0;
        while start < order.len() {
            let id = ids[order[start] as usize];
            let mut end = start + 1;
            while end < order.len() && same(id, ids[order[end] as usize]) {
                end += 1;
            }
            let clips = chain_rects(&chains, id);
            let mark = out.len();
            out.extend(clips.iter().map(|&rect| PaintCommand::PushClip { rect }));
            let inner = out.len();
            paint_run(layout, &order[start..end], &mut out);
            if out.len() == inner {
                // The run painted nothing: drop its clip pairs instead of dispatching them.
                out.truncate(mark);
            } else {
                out.extend(clips.iter().map(|_| PaintCommand::PopClip));
            }
            start = end;
        }
    }
    layout.z_order = order;
    out
}

/// Whether two chains hold the same rectangles in the same order (outermost first). Walks both
/// chains from the innermost link, so it returns the same answer as comparing their full lists.
fn same_chain(chains: &[ClipChain], mut a: u32, mut b: u32) -> bool {
    loop {
        if a == b {
            return true;
        }
        let (ca, cb) = (&chains[a as usize], &chains[b as usize]);
        if ca.depth != cb.depth || ca.rect != cb.rect {
            return false;
        }
        if ca.depth == 0 {
            return true;
        }
        a = ca.parent;
        b = cb.parent;
    }
}

/// The rectangles of a chain, outermost first.
fn chain_rects(chains: &[ClipChain], mut id: u32) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(chains[id as usize].depth as usize);
    while chains[id as usize].depth > 0 {
        rects.push(chains[id as usize].rect);
        id = chains[id as usize].parent;
    }
    rects.reverse();
    rects
}

/// Paint commands with each scroll container's descendants clipped to its
/// viewport. `layout` must already be translated. Full rebuild: the reference
/// that `build_scrolled_paint_cached` must match.
#[cfg(test)]
pub(crate) fn build_scrolled_paint(
    document: &RuntimeDocument,
    layout: &mut LayoutResult,
    interaction: Interaction,
) -> Vec<PaintCommand> {
    build_clipped(document, layout, interaction, |layout, nodes, out| {
        layout.z_order.clear();
        layout.z_order.extend_from_slice(nodes);
        out.extend(build_paint_commands_with_scroll(
            document,
            layout,
            interaction,
            &|i| document.scroll_offset(i),
        ));
    })
}

/// Per-node paint memo kept across frames by the window. A node's emitted commands are a
/// pure function of the inputs captured in `NodeKey` plus the document's colour variables,
/// so replaying a memoised run yields exactly what a full rebuild would emit.
#[derive(Default)]
pub(crate) struct PaintNodeCache {
    variables: Vec<Color>,
    /// Boxed: most nodes of a large pooled document are hidden and keep only an empty slot.
    nodes: Vec<Option<Box<NodeEntry>>>,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
}

struct NodeEntry {
    key: NodeKey,
    commands: Vec<PaintCommand>,
}

/// Every input the emission of one visible node reads: its resolved style, layout box and content
/// extent, text, node backdrop, and its own scroll state. Hidden nodes (including ancestors) emit
/// nothing and are not memoised. Image nodes are never memoised because their emission reads asset
/// pixels and revisions.
struct NodeKey {
    style: Style,
    rect: Rect,
    content: Rect,
    text: String,
    blur: Option<u8>,
    scroll: ScrollState,
}

fn node_rect(layout: &LayoutResult, index: u32) -> Rect {
    layout
        .boxes
        .get(index as usize)
        .map(|entry| entry.rect)
        .unwrap_or_default()
}

impl NodeKey {
    fn capture(
        document: &RuntimeDocument,
        layout: &LayoutResult,
        interaction: Interaction,
        index: u32,
    ) -> Self {
        Self {
            style: document.runtime_style(index, interaction),
            rect: node_rect(layout, index),
            content: layout.content_extent(index),
            text: document.text_for(index).to_owned(),
            blur: document.node_backdrop_blur(index),
            scroll: document.scroll_offset(index),
        }
    }

    fn matches(
        &self,
        document: &RuntimeDocument,
        layout: &LayoutResult,
        interaction: Interaction,
        index: u32,
    ) -> bool {
        self.blur == document.node_backdrop_blur(index)
            && self.scroll == document.scroll_offset(index)
            && self.rect == node_rect(layout, index)
            && self.content == layout.content_extent(index)
            && self.text == document.text_for(index)
            && self.style == document.runtime_style(index, interaction)
    }
}

/// Appends the commands of `nodes` (a contiguous z-order run), replaying each memoised node
/// whose inputs are unchanged and re-emitting only the others, one node at a time.
fn paint_memo(
    document: &RuntimeDocument,
    cache: &mut PaintNodeCache,
    interaction: Interaction,
    layout: &mut LayoutResult,
    nodes: &[u32],
    out: &mut Vec<PaintCommand>,
) {
    for &index in nodes {
        let slot = &mut cache.nodes[index as usize];
        if !document.is_effectively_visible(index) {
            // Paint skips hidden nodes before resolving anything else: nothing to emit or keep.
            *slot = None;
            continue;
        }
        if let Some(entry) = slot
            .as_ref()
            .filter(|entry| entry.key.matches(document, layout, interaction, index))
        {
            out.extend_from_slice(&entry.commands);
            #[cfg(test)]
            {
                cache.hits += 1;
            }
            continue;
        }
        #[cfg(test)]
        {
            cache.misses += 1;
        }
        let key = (document.document.nodes[index as usize].kind != NodeKind::Image)
            .then(|| NodeKey::capture(document, layout, interaction, index));
        layout.z_order.clear();
        layout.z_order.push(index);
        let commands = build_paint_commands_with_scroll(document, layout, interaction, &|i| {
            document.scroll_offset(i)
        });
        match key {
            Some(key) => {
                out.extend_from_slice(&commands);
                *slot = Some(Box::new(NodeEntry { key, commands }));
            }
            None => {
                out.extend(commands);
                *slot = None;
            }
        }
    }
}

/// Same output as `build_scrolled_paint`, re-emitting only the nodes whose inputs changed
/// since the previous call that used `cache`.
pub(crate) fn build_scrolled_paint_cached(
    document: &RuntimeDocument,
    layout: &mut LayoutResult,
    interaction: Interaction,
    cache: &mut PaintNodeCache,
) -> Vec<PaintCommand> {
    let count = document.document.nodes.len();
    if cache.nodes.len() != count || cache.variables != document.variable_values {
        cache.nodes.clear();
        cache.nodes.resize_with(count, || None);
        cache.variables.clone_from(&document.variable_values);
    }
    build_clipped(document, layout, interaction, |layout, nodes, out| {
        paint_memo(document, cache, interaction, layout, nodes, out);
    })
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
        let mut scroller = Style {
            width: Length::Px(300.0),
            overflow_y: Overflow::Auto,
            ..Style::default()
        };
        if let Some(h) = height {
            scroller.height = Length::Px(h);
        }
        let mut tall = Style {
            width: Length::Px(180.0),
            height: Length::Px(500.0),
            ..Style::default()
        };
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
        doc.apply_scroll_delta(
            0,
            ScrollDelta {
                dx: 0.0,
                dy: 1000.0,
            },
            (0.0, max_y),
        );
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

    const BENCH_ROWS: usize = 42;
    // Mirrors the terminal UI run pool (flamewm-terminal/src/pool.rs); this crate cannot import it.
    const POOL_ROWS: usize = 96;
    const POOL_RUNS: usize = 28;

    fn run_id(row: usize, slot: usize) -> String {
        format!("r{row}s{slot}")
    }

    /// Terminal-shaped document: a root holding the whole run pool (`POOL_ROWS` x `POOL_RUNS`
    /// absolutely positioned text nodes, ids as the UI pool emits them). Rows from
    /// `visible_rows` on are hidden, like unused pool slots.
    fn terminal_fixture(visible_rows: usize) -> RuntimeDocument {
        pool_fixture(POOL_ROWS, POOL_RUNS, visible_rows, 700.0)
    }

    /// `pool_fixture` with the terminal's pool shape and an overflowing root (`root_height`).
    fn pool_fixture(
        rows: usize,
        slots: usize,
        visible_rows: usize,
        root_height: f32,
    ) -> RuntimeDocument {
        let root = Style {
            width: Length::Px(800.0),
            height: Length::Px(root_height),
            overflow_y: Overflow::Auto,
            ..Style::default()
        };
        let mut nodes = vec![CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: Some(1),
            next_sibling: None,
            id: std::borrow::Cow::Borrowed("root"),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: None,
            style: root,
            hover_style: None,
            active_style: None,
        }];
        let count = rows * slots;
        for pool in 0..count {
            let (row, slot) = (pool / slots, pool % slots);
            let mut style = Style {
                position: flamewm_render_core::Position::Absolute,
                left: Length::Px((slot % 4) as f32 * 180.0),
                top: Length::Px(row as f32 * 16.0),
                width: Length::Px(170.0),
                height: Length::Px(16.0),
                ..Style::default()
            };
            if slot % 2 == 0 {
                style.background = flamewm_render_core::ColorValue::Variable(0);
            }
            if slot % 3 == 0 {
                style.color = flamewm_render_core::ColorValue::Variable(1);
            }
            let mut hover = style.clone();
            hover.background = flamewm_render_core::ColorValue::Literal(Color::rgb(200, 0, 0));
            nodes.push(CompiledNode {
                kind: NodeKind::Text,
                parent: Some(0),
                first_child: None,
                next_sibling: (pool + 1 < count).then_some(pool as u32 + 2),
                id: std::borrow::Cow::Owned(run_id(row, slot)),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Owned(format!("row {row:02} run {slot:02} lorem")),
                image: None,
                style,
                hover_style: Some(Box::new(hover)),
                active_style: None,
            });
        }
        let mut doc = RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: vec![
                flamewm_render_core::ColorVariable {
                    name: std::borrow::Cow::Borrowed("accent"),
                    default: Color::rgb(10, 120, 200),
                },
                flamewm_render_core::ColorVariable {
                    name: std::borrow::Cow::Borrowed("muted"),
                    default: Color::rgb(90, 90, 90),
                },
            ],
            assets: Vec::new(),
            nodes,
        })
        .expect("terminal fixture is a valid document");
        for pool in 0..count {
            let (row, slot) = (pool / slots, pool % slots);
            if row >= visible_rows {
                doc.set_visible(&run_id(row, slot), false)
                    .expect("hide unused run");
            }
        }
        doc
    }

    /// Debug timing of the paint-command build under a one-character edit per frame: the full
    /// rebuild (reference) and the node-memo build, over the same edit sequence.
    /// Run: `cargo test -p flamewm-render-x11 scroll::tests::bench_one_char -- --ignored --nocapture`
    #[test]
    #[ignore = "debug timing; run with --ignored --nocapture"]
    fn bench_one_char_edit_paint_build() {
        let it = Interaction::default();
        const FRAMES: usize = 60;
        let (mut full, mut cached) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        let mut commands = 0usize;
        let mut doc = terminal_fixture(BENCH_ROWS);
        let mut cache = PaintNodeCache::default();
        for frame in 0..FRAMES {
            let row = frame % BENCH_ROWS;
            doc.set_text(
                &run_id(row, 0),
                format!("edit {frame:04} row {row:02} lorem"),
            )
            .expect("edit run text");
            let mut layout = LayoutEngine::compute(&doc, 800.0, 700.0, it);
            fit_scroll_extents(&mut layout, &doc, it);
            translate_scrolled_content(&mut layout, &doc, it);
            let start = std::time::Instant::now();
            let reference = build_scrolled_paint(&doc, &mut layout, it);
            full += start.elapsed();
            let start = std::time::Instant::now();
            let built = build_scrolled_paint_cached(&doc, &mut layout, it, &mut cache);
            cached += start.elapsed();
            assert!(built == reference, "frame {frame}: memo build diverged");
            commands += built.len();
            std::hint::black_box(&built);
        }
        let per = |spent: std::time::Duration| spent.as_secs_f64() * 1000.0 / FRAMES as f64;
        eprintln!(
            "build_scrolled_paint (debug, one-char edit, {FRAMES} frames, {} commands/frame): full rebuild avg {:.3} ms/frame, node memo avg {:.3} ms/frame",
            commands / FRAMES,
            per(full),
            per(cached)
        );
    }

    /// Deterministic xorshift64 so the differential sequences are reproducible.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn color(&mut self) -> Color {
            Color {
                r: self.below(256) as u8,
                g: self.below(256) as u8,
                b: self.below(256) as u8,
                a: self.below(256) as u8,
            }
        }
    }

    /// Builds the layout exactly as the window's redraw does (fit, then translate).
    fn fresh_layout(
        doc: &RuntimeDocument,
        width: f32,
        height: f32,
        it: Interaction,
    ) -> LayoutResult {
        let mut layout = LayoutEngine::compute(doc, width, height, it);
        fit_scroll_extents(&mut layout, doc, it);
        translate_scrolled_content(&mut layout, doc, it);
        layout
    }

    /// Random edits (single-character text, scroll, background/foreground/backdrop/visibility,
    /// resize, colour variable, interaction hover) against one persistent memo. After every
    /// edit the memo build must equal a full rebuild, and a repeated build with no edit must too.
    #[test]
    fn node_memo_matches_full_rebuild_over_random_edits() {
        const ROWS: usize = 8;
        const SLOTS: usize = 6;
        const STEPS: usize = 400;
        let mut doc = pool_fixture(ROWS, SLOTS, ROWS, 60.0);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let (mut width, mut height) = (800.0f32, 60.0f32);
        let mut it = Interaction::default();
        let mut cache = PaintNodeCache::default();
        let mut text_edit_misses = 0usize;
        let mut text_edits = 0usize;
        let mut extra = Vec::new();
        for step in 0..STEPS {
            let (row, slot) = (rng.below(ROWS), rng.below(SLOTS));
            let id = run_id(row, slot);
            let mut is_text_edit = false;
            let op = rng.below(10);
            match op {
                0 | 1 => {
                    is_text_edit = true;
                    let glyph = (b'a' + rng.below(26) as u8) as char;
                    let len = 1 + rng.below(12);
                    doc.set_text(&id, glyph.to_string().repeat(len))
                        .expect("text");
                }
                2 => {
                    let dy = rng.below(81) as f32 - 40.0;
                    doc.apply_scroll_delta(
                        0,
                        flamewm_render_core::ScrollDelta { dx: 0.0, dy },
                        (0.0, 400.0),
                    );
                }
                3 => doc
                    .set_background_color(&id, rng.color())
                    .expect("background"),
                4 => doc
                    .set_foreground_color(
                        &id,
                        flamewm_render_core::ColorValue::Literal(rng.color()),
                    )
                    .expect("foreground"),
                5 => doc.set_visible(&id, rng.below(2) == 0).expect("visible"),
                6 => {
                    let blur = (rng.below(2) == 0).then(|| 1 + rng.below(100) as u8);
                    doc.set_node_backdrop(&id, blur).expect("backdrop");
                }
                7 => {
                    width = 300.0 + rng.below(600) as f32;
                    height = 40.0 + rng.below(660) as f32;
                }
                8 => {
                    let name = if rng.below(2) == 0 { "accent" } else { "muted" };
                    doc.set_color_variable(name, rng.color()).expect("variable");
                }
                _ => {
                    // Hover moves the resolved style of one node (the hovered run gets a hover style).
                    let hover = (rng.below(2) == 0).then(|| (row * SLOTS + slot) as u32 + 1);
                    it = Interaction {
                        hover,
                        ..Interaction::default()
                    };
                }
            }
            let mut layout = fresh_layout(&doc, width, height, it);
            let reference = build_scrolled_paint(&doc, &mut layout, it);
            let before = cache.misses;
            let built = build_scrolled_paint_cached(&doc, &mut layout, it, &mut cache);
            assert!(
                built == reference,
                "step {step}: memo build diverged from full rebuild"
            );
            let again = build_scrolled_paint_cached(&doc, &mut layout, it, &mut cache);
            assert!(
                again == reference,
                "step {step}: repeated memo build diverged"
            );
            // Step 0 is the cold start (empty memo): every node emits once there.
            if is_text_edit && step > 0 {
                text_edits += 1;
                let missed = cache.misses - before;
                text_edit_misses += missed;
                if missed > 1 {
                    extra.push(format!("step {step} op {op} misses {missed}"));
                }
            }
        }
        assert!(
            cache.hits > cache.misses,
            "memo must replay most nodes, got {} hits / {} misses",
            cache.hits,
            cache.misses
        );
        assert!(
            extra.is_empty(),
            "a single-character edit must re-emit only its own node ({text_edit_misses} misses over {text_edits} edits): {extra:?}"
        );
    }

    /// A colour-variable change must reach memoised runs that resolve their colour through it.
    #[test]
    fn node_memo_follows_colour_variable_changes() {
        let mut doc = pool_fixture(2, 2, 2, 200.0);
        let it = Interaction::default();
        let mut cache = PaintNodeCache::default();
        let mut layout = fresh_layout(&doc, 800.0, 200.0, it);
        let first = build_scrolled_paint_cached(&doc, &mut layout, it, &mut cache);
        assert!(first == build_scrolled_paint(&doc, &mut layout, it));

        // Slot 0 of row 0 takes `muted` as its text colour (slot % 3 == 0).
        let muted = Color::rgb(7, 8, 9);
        doc.set_color_variable("muted", muted).expect("variable");
        let mut layout = fresh_layout(&doc, 800.0, 200.0, it);
        let reference = build_scrolled_paint(&doc, &mut layout, it);
        let built = build_scrolled_paint_cached(&doc, &mut layout, it, &mut cache);
        assert!(
            built == reference,
            "memo kept a colour from before the variable changed"
        );
        assert!(
            reference
                .iter()
                .any(|c| matches!(c, PaintCommand::Text { color, .. } if *color == muted)),
            "the changed variable colour is painted"
        );
    }
}
