use super::*;

/// Root (index 0) holding `rows` element rows; row `i` is element `1 + 2i` with one text child `2 + 2i`.
fn rows_document(rows: &[(String, Style)]) -> RuntimeDocument {
    let count = rows.len() as u32;
    let mut nodes = vec![CompiledNode {
        kind: NodeKind::Element,
        parent: None,
        first_child: (count > 0).then_some(1),
        next_sibling: None,
        id: "root".to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: "".to_string().into(),
        image: None,
        style: Style::default(),
        hover_style: None,
        active_style: None,
    }];
    for (i, (text, style)) in rows.iter().enumerate() {
        let row = 1 + 2 * i as u32;
        let text_index = row + 1;
        nodes.push(CompiledNode {
            kind: NodeKind::Element,
            parent: Some(0),
            first_child: Some(text_index),
            next_sibling: (i + 1 < rows.len()).then_some(row + 2),
            id: format!("row{i}").into(),
            action: std::borrow::Cow::Borrowed(""),
            text: "".to_string().into(),
            image: None,
            style: style.clone(),
            hover_style: None,
            active_style: None,
        });
        nodes.push(CompiledNode {
            kind: NodeKind::Text,
            parent: Some(row),
            first_child: None,
            next_sibling: None,
            id: "".to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: text.clone().into(),
            image: None,
            style: Style::default(),
            hover_style: None,
            active_style: None,
        });
    }
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes,
    })
    .expect("rows document validates")
}

fn geometry_node(
    kind: NodeKind,
    parent: Option<u32>,
    first_child: Option<u32>,
    next_sibling: Option<u32>,
    id: &str,
    style: Style,
) -> CompiledNode {
    CompiledNode {
        kind,
        parent,
        first_child,
        next_sibling,
        id: id.to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: "".to_string().into(),
        image: None,
        style,
        hover_style: None,
        active_style: None,
    }
}

/// Absolute box of `px` by `px` at the origin; flow nodes use the defaults.
fn absolute_box(width: f32, height: f32) -> Style {
    Style {
        position: Position::Absolute,
        width: Length::Px(width),
        height: Length::Px(height),
        ..Style::default()
    }
}

/// root > [panel > text, overlay (abs leaf), frame (abs, padded, hidden) > badge (abs leaf),
/// scroller (overflow auto) > inner (abs leaf)]. `overlay`/`badge` take the patch path;
/// `inner` sits under a scrolling ancestor and must fall back to a full compute.
fn geometry_document() -> RuntimeDocument {
    let mut frame = absolute_box(200.0, 150.0);
    frame.padding = Edges {
        top: 6.0,
        right: 6.0,
        bottom: 6.0,
        left: 6.0,
    };
    frame.overflow_x = Overflow::Hidden;
    frame.overflow_y = Overflow::Hidden;
    let mut scroller = Style::default();
    scroller.overflow_x = Overflow::Hidden;
    scroller.overflow_y = Overflow::Auto;
    let nodes = vec![
        geometry_node(
            NodeKind::Element,
            None,
            Some(1),
            None,
            "root",
            Style::default(),
        ),
        geometry_node(
            NodeKind::Element,
            Some(0),
            Some(2),
            Some(3),
            "panel",
            Style::default(),
        ),
        geometry_node(NodeKind::Text, Some(1), None, None, "", Style::default()),
        geometry_node(
            NodeKind::Element,
            Some(0),
            None,
            Some(4),
            "overlay",
            absolute_box(120.0, 80.0),
        ),
        geometry_node(NodeKind::Element, Some(0), Some(5), Some(6), "frame", frame),
        geometry_node(
            NodeKind::Element,
            Some(4),
            None,
            None,
            "badge",
            absolute_box(20.0, 20.0),
        ),
        geometry_node(
            NodeKind::Element,
            Some(0),
            Some(7),
            None,
            "scroller",
            scroller,
        ),
        geometry_node(
            NodeKind::Element,
            Some(6),
            None,
            None,
            "inner",
            absolute_box(40.0, 30.0),
        ),
    ];
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes,
    })
    .expect("geometry document validates")
}

fn assert_same_layout(patched: &LayoutResult, full: &LayoutResult) {
    assert_eq!(
        patched.boxes, full.boxes,
        "patched boxes must equal full compute"
    );
    assert_eq!(
        patched.contents, full.contents,
        "patched contents must equal full compute"
    );
    assert_eq!(patched.z_order, full.z_order);
}

#[test]
fn isolated_geometry_patch_matches_full_compute_over_random_moves() {
    let interaction = InteractionState::default();
    let mut doc = geometry_document();
    let mut layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);
    let layout_revision = doc.layout_revision();
    // Fixed-seed LCG: a deterministic stream of overlay and badge moves and resizes.
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = |bound: f32| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32 / (1u64 << 31) as f32) * bound
    };
    for step in 0..400 {
        let target = if step % 3 == 0 { "badge" } else { "overlay" };
        if step % 5 == 4 {
            let (width, height) = (next(160.0) + 1.0, next(120.0) + 1.0);
            doc.set_size_px(target, width, height).expect("size");
        } else {
            doc.set_position_px(target, next(700.0), next(500.0))
                .expect("move");
        }
        let passes = layout_passes_on_this_thread();
        assert!(
            doc.patch_layout(&mut layout, interaction),
            "step {step}: isolated move must patch"
        );
        assert_eq!(
            layout_passes_on_this_thread(),
            passes,
            "patch must not run a full compute"
        );
        assert_eq!(
            doc.layout_revision(),
            layout_revision,
            "isolated moves keep layout revision"
        );
        assert_eq!(layout.revision, doc.revision());
        assert_same_layout(
            &layout,
            &LayoutEngine::compute(&doc, 800.0, 600.0, interaction),
        );
    }
}

#[test]
fn patch_falls_back_to_full_compute_when_not_provably_safe() {
    let interaction = InteractionState::default();
    let mut doc = geometry_document();
    let mut layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);

    // Under a scrolling ancestor: the scroller's content offset is not modelled by the patch.
    doc.set_position_px("inner", 10.0, 12.0)
        .expect("inner move");
    assert!(!doc.patch_layout(&mut layout, interaction));
    layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);
    assert_same_layout(
        &layout,
        &LayoutEngine::compute(&doc, 800.0, 600.0, interaction),
    );

    // In-flow geometry moves the layout revision: never patched.
    doc.set_position_px("panel", 3.0, 3.0).expect("flow move");
    assert!(!doc.patch_layout(&mut layout, interaction));
    layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);
    // After the full compute the next isolated move patches again.
    doc.set_position_px("overlay", 300.0, 40.0)
        .expect("overlay move");
    assert!(doc.patch_layout(&mut layout, interaction));
    assert_same_layout(
        &layout,
        &LayoutEngine::compute(&doc, 800.0, 600.0, interaction),
    );
}

/// Terminal-like runs: absolutely positioned boxes holding one text node each, some sized by their
/// text. Text, visibility, weight and position writes patch the retained layout, and the result
/// equals a full compute at every step.
#[test]
fn isolated_text_box_patch_matches_full_compute() {
    let interaction = InteractionState::default();
    let rows: Vec<(String, Style)> = (0..12)
        .map(|i| {
            let style = if i % 2 == 0 {
                absolute_box(80.0, 16.0)
            } else {
                Style {
                    position: Position::Absolute,
                    ..Style::default()
                }
            };
            (format!("cell {i}"), style)
        })
        .collect();
    let mut doc = rows_document(&rows);
    let mut layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);
    let layout_revision = doc.layout_revision();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = |bound: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % bound
    };
    for step in 0..400 {
        let row = format!("row{}", next(12));
        match step % 4 {
            0 => doc
                .set_text(&row, "x".repeat(next(40) as usize))
                .expect("text"),
            1 => doc.set_visible(&row, next(3) != 0).expect("visibility"),
            2 => doc
                .set_position_px(&row, next(700) as f32, next(500) as f32)
                .expect("move"),
            _ => doc
                .set_font_weight(&row, if next(2) == 0 { 400 } else { 700 })
                .expect("weight"),
        }
        let passes = layout_passes_on_this_thread();
        assert!(
            doc.patch_layout(&mut layout, interaction),
            "step {step}: isolated text box must patch"
        );
        assert_eq!(layout_passes_on_this_thread(), passes);
        assert_eq!(doc.layout_revision(), layout_revision);
        assert_eq!(layout.revision, doc.revision());
        assert_same_layout(
            &layout,
            &LayoutEngine::compute(&doc, 800.0, 600.0, interaction),
        );
    }
}

/// Text in normal flow resizes its parent and pushes its siblings: never patched.
#[test]
fn flow_text_change_invalidates_layout() {
    let interaction = InteractionState::default();
    let mut doc = rows_document(&bench_rows(3));
    let mut layout = LayoutEngine::compute(&doc, 800.0, 600.0, interaction);
    let layout_revision = doc.layout_revision();
    doc.set_text(
        "row1",
        "a much longer line that wraps differently than before",
    )
    .expect("text");
    doc.set_visible("row2", false).expect("visibility");
    assert_ne!(doc.layout_revision(), layout_revision);
    assert!(!doc.patch_layout(&mut layout, interaction));
}

fn bench_rows(count: usize) -> Vec<(String, Style)> {
    (0..count)
        .map(|i| {
            let text = format!(
                "row {i}: the quick brown fox jumps over the lazy dog {}",
                i * 7
            );
            let style = Style {
                text_wrap: TextWrap::Wrap,
                font_size: 13.0,
                ..Style::default()
            };
            (text, style)
        })
        .collect()
}

/// Diagnostic timing, not a pass/fail gate:
/// `cargo test -p flamewm-render-core --locked layout_compute_bench -- --ignored --nocapture`
#[test]
#[ignore]
fn layout_compute_bench() {
    let mut doc = rows_document(&bench_rows(300));
    let interaction = InteractionState::default();
    let iterations = 50u32;

    let start = std::time::Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(LayoutEngine::compute(&doc, 900.0, 800.0, interaction));
    }
    let full = start.elapsed() / iterations;

    let start = std::time::Instant::now();
    for i in 0..iterations {
        let text = format!("row 7: edited {i} with a different length {}", i * 13);
        doc.set_text("row7", text).expect("edit");
        std::hint::black_box(LayoutEngine::compute(&doc, 900.0, 800.0, interaction));
    }
    let edit = start.elapsed() / iterations;
    println!(
        "layout_compute_bench 300 text rows: compute(unchanged)={full:?} edit+compute={edit:?} profile={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
}

/// A same-width text edit keeps every box, so damage stays Text-only; a size change is still resolved.
#[test]
fn text_edit_damage_follows_box_change() {
    let mut doc = rows_document(&bench_rows(20));
    let interaction = InteractionState::default();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 900.0,
        height: 800.0,
    };
    // Construction latches a full dirty state; flush it so only the edits below are tracked.
    let initial = LayoutEngine::compute(&doc, 900.0, 800.0, interaction);
    let _ = doc.take_incremental_damage(&initial, &initial, interaction, viewport);

    let old = LayoutEngine::compute(&doc, 900.0, 800.0, interaction);
    doc.set_text(
        "row3",
        "row 4: the quick brown fox jumps over the lazy dog 21",
    )
    .expect("same-width edit");
    let new = LayoutEngine::compute(&doc, 900.0, 800.0, interaction);
    assert_eq!(old.boxes, new.boxes, "same-width edit must not move boxes");
    assert!(matches!(
        doc.take_incremental_damage(&old, &new, interaction, viewport),
        SurfaceDamage::Region(_)
    ));

    let old = new;
    doc.set_text("row3", "row 3\nline two\nline three")
        .expect("size-changing edit");
    let new = LayoutEngine::compute(&doc, 900.0, 800.0, interaction);
    assert_ne!(old.boxes, new.boxes, "extra lines must change the row box");
    assert_ne!(
        doc.take_incremental_damage(&old, &new, interaction, viewport),
        SurfaceDamage::None
    );
}
