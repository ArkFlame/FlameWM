use super::*;

fn text_node(parent: u32, next: Option<u32>, text: &str) -> CompiledNode {
    CompiledNode {
        kind: NodeKind::Text,
        parent: Some(parent),
        first_child: None,
        next_sibling: next,
        id: std::borrow::Cow::Borrowed(""),
        action: std::borrow::Cow::Borrowed(""),
        text: text.to_string().into(),
        image: None,
        style: Style::default().into(),
        hover_style: None,
        active_style: None,
        focus_style: None,
    }
}

#[test]
fn max_width_box_measures_text_at_its_clamped_width() {
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let box_style = Style {
        max_width: Length::Px(100.0),
        font_size: 14.0,
        ..Style::default()
    };
    let node = |parent, first_child, next_sibling, kind, id: &str, style: Style, text: &str| {
        CompiledNode {
            kind,
            parent,
            first_child,
            next_sibling,
            id: id.to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: text.to_string().into(),
            image: None,
            style: style.into(),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }
    };
    let document = CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            node(
                None,
                Some(1),
                None,
                NodeKind::Element,
                "",
                Style::default(),
                "",
            ),
            node(
                Some(0),
                Some(2),
                None,
                NodeKind::Element,
                "box",
                box_style,
                "",
            ),
            node(
                Some(1),
                None,
                None,
                NodeKind::Text,
                "",
                Style {
                    text_wrap: crate::model::TextWrap::Wrap,
                    font_size: 14.0,
                    ..Style::default()
                },
                text,
            ),
        ],
    };
    let doc = RuntimeDocument::new(document).unwrap();
    let boxed = doc.node_by_id("box").expect("box node exists");
    let size = LayoutEngine::measure_node(&doc, boxed, 800.0, 600.0, InteractionState::default())
        .expect("box measures");
    // Paint wraps the text at the box's 100 px width, so the measured height must match that wrap.
    let painted =
        crate::text_layout::layout_text(text, 14.0, 100.0, crate::model::TextWrap::Wrap, false);
    assert!(size.width <= 100.0);
    assert_eq!(size.height, painted.height);
    assert!(painted.lines.len() > 1);
}

#[test]
fn flex_grow_fills_available_width() {
    let left = Style {
        flex_grow: 1.0,
        ..Style::default()
    };
    let right = Style {
        width: Length::Px(20.0),
        ..Style::default()
    };
    let root_style = Style {
        display: Display::Flex,
        height: Length::Px(40.0),
        ..Style::default()
    };
    let document = CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: Some(1),
                next_sibling: None,
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: root_style.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
            CompiledNode {
                kind: NodeKind::Element,
                parent: Some(0),
                first_child: Some(3),
                next_sibling: Some(2),
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: left.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
            CompiledNode {
                kind: NodeKind::Element,
                parent: Some(0),
                first_child: None,
                next_sibling: None,
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: right.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
            text_node(1, None, "left"),
        ],
    };
    let runtime = RuntimeDocument::new(document).unwrap();
    let layout = LayoutEngine::compute(&runtime, 100.0, 40.0, InteractionState::default());
    assert!((layout.boxes[1].rect.width - 80.0).abs() < 0.01);
}

fn metrics(viewport_w: f32, content_w: f32) -> ScrollMetrics {
    LayoutResult::scroll_metrics(
        Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_w,
            height: 20.0,
        },
        Rect {
            x: 0.0,
            y: 0.0,
            width: content_w,
            height: 40.0,
        },
        ScrollState::default(),
    )
}

#[test]
fn thumb_tiny_tracks_never_panic() {
    for track in [1.0f32, 11.0, 12.0, 100.0] {
        let m = metrics(track, track * 4.0);
        assert!(m.thumb_x.width.is_finite());
        assert!(m.thumb_x.width <= track + 0.001);
        if track < 12.0 {
            assert!((m.thumb_x.width - track).abs() < 0.01);
        }
    }
    // Nonfinite / zero / non-scrollable all yield default thumb.
    for (vw, cw) in [
        (f32::NAN, 100.0),
        (100.0, f32::INFINITY),
        (0.0, 100.0),
        (100.0, 50.0),
        (100.0, 100.0),
        (-10.0, 100.0),
    ] {
        let m = metrics(vw, cw);
        assert_eq!(m.thumb_x, Rect::default());
    }
}

fn fixed_doc(w: f32, h: f32) -> RuntimeDocument {
    let root_style = Style {
        width: Length::Px(w),
        height: Length::Px(h),
        ..Style::default()
    };
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: std::borrow::Cow::Borrowed(""),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: None,
            style: root_style.into(),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }],
    })
    .unwrap()
}

fn overflow_auto_doc() -> RuntimeDocument {
    let root_style = Style {
        width: Length::Px(200.0),
        height: Length::Px(100.0),
        overflow_y: Overflow::Auto,
        ..Style::default()
    };
    let child_style = Style {
        width: Length::Px(180.0),
        height: Length::Px(500.0),
        ..Style::default()
    };
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: Some(1),
                next_sibling: None,
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: root_style.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
            CompiledNode {
                kind: NodeKind::Element,
                parent: Some(0),
                first_child: None,
                next_sibling: None,
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: child_style.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
        ],
    })
    .unwrap()
}

fn popup_wrapper_doc() -> RuntimeDocument {
    let wrapper_style = Style {
        width: Length::Percent(1.0),
        height: Length::Percent(1.0),
        ..Style::default()
    };
    let popup_style = Style {
        width: Length::Px(200.0),
        height: Length::Px(100.0),
        ..Style::default()
    };
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: Some(1),
                next_sibling: None,
                id: "wrapper".into(),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: wrapper_style.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
            CompiledNode {
                kind: NodeKind::Element,
                parent: Some(0),
                first_child: None,
                next_sibling: None,
                id: "popup".into(),
                action: std::borrow::Cow::Borrowed(""),
                text: std::borrow::Cow::Borrowed(""),
                image: None,
                style: popup_style.into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            },
        ],
    })
    .unwrap()
}

#[test]
fn measure_root_finite_fixed_popup() {
    let doc = fixed_doc(200.0, 100.0);
    let size = LayoutEngine::measure_root(&doc, 800.0, 600.0, InteractionState::default())
        .expect("finite fixed popup measures");
    assert_eq!((size.width, size.height), (200.0, 100.0));
}

#[test]
fn measure_root_overflow_auto_child_not_max() {
    let doc = overflow_auto_doc();
    let size = LayoutEngine::measure_root(&doc, 800.0, 600.0, InteractionState::default())
        .expect("overflow:auto root measures");
    assert!(size.width.is_finite() && size.height.is_finite());
    assert!(size.width < f32::MAX / 2.0 && size.height < f32::MAX / 2.0);
    assert!(size.width <= 800.0 && size.height <= 600.0);
    assert_eq!((size.width, size.height), (200.0, 100.0));
}

#[test]
fn measure_node_selects_compact_popup_over_constraint_sized_wrapper() {
    let doc = popup_wrapper_doc();
    let popup = doc.node_by_id("popup").expect("popup node exists");
    let interaction = InteractionState::default();
    let popup_size = LayoutEngine::measure_node(&doc, popup, 800.0, 600.0, interaction)
        .expect("fixed popup measures");
    let root_size =
        LayoutEngine::measure_root(&doc, 800.0, 600.0, interaction).expect("wrapper root measures");
    assert_eq!((popup_size.width, popup_size.height), (200.0, 100.0));
    assert_eq!((root_size.width, root_size.height), (800.0, 600.0));
}

/// Nested flow/flex/absolute/display:none/scroll fixture. Child order is
/// the sibling-link order, including absolute and display:none children.
fn traversal_fixture() -> RuntimeDocument {
    let node = |parent: Option<u32>,
                first_child: Option<u32>,
                next_sibling: Option<u32>,
                kind: NodeKind,
                text: &str,
                style: Style| CompiledNode {
        kind,
        parent,
        first_child,
        next_sibling,
        id: std::borrow::Cow::Borrowed(""),
        action: std::borrow::Cow::Borrowed(""),
        text: text.to_string().into(),
        image: None,
        style: style.into(),
        hover_style: None,
        active_style: None,
        focus_style: None,
    };
    let px = |w: f32, h: f32| Style {
        width: Length::Px(w),
        height: Length::Px(h),
        ..Style::default()
    };
    let mut root = px(300.0, 200.0);
    root.overflow_y = Overflow::Auto;
    let mut row = px(120.0, 40.0);
    row.display = Display::Flex;
    row.flex_direction = FlexDirection::Row;
    let mut abs = px(50.0, 30.0);
    abs.position = Position::Absolute;
    abs.right = Length::Px(0.0);
    abs.top = Length::Px(0.0);
    let mut hidden = px(10.0, 10.0);
    hidden.display = Display::None;
    let hidden_leaf = Style {
        display: Display::None,
        ..Style::default()
    };
    let col = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        height: Length::Px(60.0),
        ..Style::default()
    };
    let grow = Style {
        flex_grow: 1.0,
        ..Style::default()
    };
    let mut inner_abs = px(15.0, 15.0);
    inner_abs.position = Position::Absolute;
    inner_abs.left = Length::Px(10.0);
    inner_abs.top = Length::Px(5.0);
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            // 0 root (scroll container): children 1, 2, 3, 4
            node(None, Some(1), None, NodeKind::Element, "", root),
            // 1 flex row: children 5, 6, 7
            node(Some(0), Some(5), Some(2), NodeKind::Element, "", row),
            // 2 absolute: no children, sibling 3
            node(Some(0), None, Some(3), NodeKind::Element, "", abs),
            // 3 display:none with a child
            node(Some(0), Some(8), Some(4), NodeKind::Element, "", hidden),
            // 4 flex column: children 9, 10
            node(Some(0), Some(9), None, NodeKind::Element, "", col),
            // 5 flow leaf
            node(
                Some(1),
                None,
                Some(6),
                NodeKind::Element,
                "",
                px(30.0, 20.0),
            ),
            // 6 absolute child of flex row
            node(Some(1), None, Some(7), NodeKind::Element, "", inner_abs),
            // 7 display:none in flex row
            node(Some(1), None, None, NodeKind::Element, "", hidden_leaf),
            // 8 child of display:none
            node(
                Some(3),
                None,
                None,
                NodeKind::Text,
                "hidden",
                Style::default(),
            ),
            // 9 grow child
            node(Some(4), None, Some(10), NodeKind::Element, "", grow),
            // 10 text
            node(Some(4), None, None, NodeKind::Text, "hi", Style::default()),
        ],
    })
    .unwrap()
}

#[test]
fn traversal_order_and_boxes_match_fixture() {
    let doc = traversal_fixture();
    let engine = Engine {
        document: &doc,
        interaction: InteractionState::default(),
        boxes: &mut [],
        measures: Default::default(),
    };
    assert_eq!(engine.children(0).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_eq!(engine.children(1).collect::<Vec<_>>(), vec![5, 6, 7]);
    assert_eq!(engine.children(3).collect::<Vec<_>>(), vec![8]);
    assert_eq!(engine.children(5).collect::<Vec<_>>(), Vec::<u32>::new());
    let layout = LayoutEngine::compute(&doc, 300.0, 200.0, InteractionState::default());
    let rects: Vec<Rect> = layout.boxes.iter().map(|b| b.rect).collect();
    // Recorded from the pre-optimisation Vec-returning traversal.
    let expected: [(f32, f32, f32, f32); 11] = [
        (0.0, 0.0, 300.0, 200.0),
        (0.0, 0.0, 120.0, 40.0),
        (250.0, 0.0, 50.0, 30.0),
        (0.0, 0.0, 0.0, 0.0),
        (0.0, 40.0, 300.0, 60.0),
        (0.0, 0.0, 30.0, 20.0),
        (10.0, 5.0, 15.0, 15.0),
        (0.0, 0.0, 0.0, 0.0),
        (0.0, 0.0, 0.0, 0.0),
        (0.0, 40.0, 300.0, 41.8),
        (0.0, 81.8, 300.0, 18.2),
    ];
    assert_eq!(rects.len(), expected.len());
    for (index, (rect, (x, y, w, h))) in rects.iter().zip(expected).enumerate() {
        assert!(
            (rect.x - x).abs() < 0.01
                && (rect.y - y).abs() < 0.01
                && (rect.width - w).abs() < 0.01
                && (rect.height - h).abs() < 0.01,
            "node {index}: got {rect:?}, expected ({x},{y},{w},{h})"
        );
    }
}

#[test]
fn measure_root_refuses_bad_constraints() {
    let doc = fixed_doc(200.0, 100.0);
    for (w, h) in [
        (f32::NAN, 100.0),
        (100.0, f32::INFINITY),
        (f32::INFINITY, f32::INFINITY),
        (0.0, 100.0),
        (100.0, 0.0),
        (-10.0, 100.0),
        (100.0, -5.0),
    ] {
        let err = LayoutEngine::measure_root(&doc, w, h, InteractionState::default())
            .expect_err("bad constraint must fail");
        assert!(
            err == IntrinsicMeasureError::NonFinite || err == IntrinsicMeasureError::NonPositive,
            "unexpected {err:?} for ({w},{h})"
        );
    }
}

#[test]
fn measure_root_result_over_constraint_refused() {
    let doc = fixed_doc(500.0, 400.0);
    let err = LayoutEngine::measure_root(&doc, 100.0, 100.0, InteractionState::default())
        .expect_err("oversize result must fail");
    assert_eq!(err, IntrinsicMeasureError::ExceedsConstraint);
}

/// A 540px flex column holding a `nav-scroll` (flex-grow 1, min-height 0, overflow-y auto) whose
/// content is 618px tall: the scroll container shrinks to the column, so its rows can scroll.
#[test]
fn scroll_container_shrinks_to_the_bounded_column() {
    let column = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        height: Length::Px(540.0),
        ..Style::default()
    };
    let scroll = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        flex_grow: 1.0,
        min_height: Length::Px(0.0),
        overflow_y: Overflow::Auto,
        ..Style::default()
    };
    let tall = Style {
        height: Length::Px(618.0),
        ..Style::default()
    };
    let element = |parent, first_child, next_sibling, style: Style| CompiledNode {
        kind: NodeKind::Element,
        parent,
        first_child,
        next_sibling,
        id: std::borrow::Cow::Borrowed(""),
        action: std::borrow::Cow::Borrowed(""),
        text: std::borrow::Cow::Borrowed(""),
        image: None,
        style: style.into(),
        hover_style: None,
        active_style: None,
        focus_style: None,
    };
    let document = CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            element(None, Some(1), None, column),
            element(Some(0), Some(2), None, scroll),
            element(Some(1), None, None, tall),
        ],
    };
    let runtime = RuntimeDocument::new(document).unwrap();
    let layout = LayoutEngine::compute(&runtime, 300.0, 540.0, InteractionState::default());
    assert!((layout.boxes[1].rect.height - 540.0).abs() < 0.01);
    assert!((layout.boxes[2].rect.height - 618.0).abs() < 0.01);
}
