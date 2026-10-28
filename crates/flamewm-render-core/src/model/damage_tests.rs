use super::*;
use crate::{LayoutEngine, LayoutResult};

const VIEW_W: f32 = 800.0;
const VIEW_H: f32 = 600.0;

fn viewport() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        width: VIEW_W,
        height: VIEW_H,
    }
}

fn node(
    kind: NodeKind,
    id: &str,
    text: &str,
    parent: Option<u32>,
    first_child: Option<u32>,
    next_sibling: Option<u32>,
    style: Style,
) -> CompiledNode {
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
    }
}

/// root(0) -> panel(1) [-> text(3)], overlay(2) absolutely positioned at (10,10) 50x40.
fn scene(panel_border: f32) -> RuntimeDocument {
    let root = Style {
        width: Length::Px(VIEW_W),
        height: Length::Px(VIEW_H),
        ..Style::default()
    };
    let panel = Style {
        width: Length::Px(300.0),
        height: Length::Px(200.0),
        border_width: panel_border,
        ..Style::default()
    };
    let overlay = Style {
        position: Position::Absolute,
        left: Length::Px(10.0),
        top: Length::Px(10.0),
        width: Length::Px(50.0),
        height: Length::Px(40.0),
        ..Style::default()
    };
    let nodes = vec![
        node(NodeKind::Element, "root", "", None, Some(1), None, root),
        node(
            NodeKind::Element,
            "panel",
            "",
            Some(0),
            Some(3),
            Some(2),
            panel,
        ),
        node(
            NodeKind::Element,
            "overlay",
            "",
            Some(0),
            None,
            None,
            overlay,
        ),
        node(
            NodeKind::Text,
            "",
            "hello",
            Some(1),
            None,
            None,
            Style::default(),
        ),
    ];
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes,
    })
    .expect("scene validates")
}

fn layout(doc: &RuntimeDocument) -> LayoutResult {
    LayoutEngine::compute(doc, VIEW_W, VIEW_H, InteractionState::default())
}

fn damage(doc: &mut RuntimeDocument, old: &LayoutResult) -> SurfaceDamage {
    let new = layout(doc);
    doc.take_incremental_damage(old, &new, InteractionState::default(), viewport())
}

/// First frame: the consumer paints in full and clears the construction-time latch.
fn prime(doc: &mut RuntimeDocument) -> LayoutResult {
    let laid = layout(doc);
    let _ = doc.take_incremental_damage(&laid, &laid, InteractionState::default(), viewport());
    laid
}

/// Whole-pixel outward rounding of a layout rect, without any halo.
fn outward(rect: Rect) -> Rect {
    let x0 = rect.x.floor();
    let y0 = rect.y.floor();
    let x1 = (rect.x + rect.width).ceil();
    let y1 = (rect.y + rect.height).ceil();
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.width).max(b.x + b.width);
    let y1 = (a.y + a.height).max(b.y + b.height);
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

#[test]
fn identical_mutation_produces_no_damage() {
    let mut doc = scene(0.0);
    let before = prime(&mut doc);
    doc.set_position_px("overlay", 10.0, 10.0)
        .expect("first write");
    doc.set_background_color("panel", Color::BLACK)
        .expect("first write");
    let _ = damage(&mut doc, &before);
    let old = layout(&doc);
    let revision = doc.revision();
    doc.set_position_px("overlay", 10.0, 10.0)
        .expect("same position");
    doc.set_background_color("panel", Color::BLACK)
        .expect("same colour");
    assert_eq!(doc.revision(), revision, "identical writes are no-ops");
    assert_eq!(damage(&mut doc, &old), SurfaceDamage::None);
}

#[test]
fn one_color_change_damages_only_node_bounds_plus_halo() {
    let mut doc = scene(2.0);
    let old = prime(&mut doc);
    let panel = old.boxes[1].rect;
    assert_eq!(
        panel,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 300.0,
            height: 200.0
        }
    );
    let layout_revision = doc.layout_revision();
    doc.set_border_color("panel", Color::BLACK).expect("border");
    assert_eq!(
        doc.layout_revision(),
        layout_revision,
        "colour is paint-only"
    );
    // Border 2px -> halo ceil(2)+1 = 3px.
    assert_eq!(
        damage(&mut doc, &old),
        SurfaceDamage::Region(Rect {
            x: 0.0,
            y: 0.0,
            width: 303.0,
            height: 203.0
        })
    );
}

#[test]
fn text_change_with_stable_box_damages_only_text_bounds() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    doc.set_text("panel", "world").expect("text");
    let new = layout(&doc);
    assert_eq!(new.boxes[1].rect, old.boxes[1].rect, "fixed panel box");
    let expected = outward(old.boxes[3].rect);
    let expected = if old.boxes[3].rect == new.boxes[3].rect {
        expected
    } else {
        union(expected, outward(new.boxes[3].rect))
    };
    assert_eq!(
        doc.take_incremental_damage(&old, &new, InteractionState::default(), viewport()),
        SurfaceDamage::Region(expected)
    );
}

#[test]
fn moved_overlay_damages_union_of_old_and_new() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    doc.set_position_px("overlay", 100.0, 60.0).expect("move");
    let new = layout(&doc);
    let expected = union(outward(old.boxes[2].rect), outward(new.boxes[2].rect));
    assert_eq!(
        doc.take_incremental_damage(&old, &new, InteractionState::default(), viewport()),
        SurfaceDamage::Region(expected)
    );
}

#[test]
fn structural_change_stays_full() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    doc.set_visible("overlay", false).expect("hide");
    assert_eq!(damage(&mut doc, &old), SurfaceDamage::Full);
}

#[test]
fn absolute_geometry_move_keeps_layout_revision_and_bumps_geometry_revision() {
    // Isolated overlay moves patch the retained layout in place: no layout revision bump,
    // one geometry revision per move.
    let mut doc = scene(0.0);
    let before = doc.layout_revision();
    let geometry_before = doc.geometry_revision();
    doc.set_position_px("overlay", 5.0, 5.0).expect("move");
    assert_eq!(doc.layout_revision(), before);
    assert_eq!(doc.geometry_revision(), geometry_before + 1);
}

#[test]
fn hundred_alternating_overlay_moves_damage_only_old_union_new() {
    let mut doc = scene(0.0);
    let mut old = prime(&mut doc);
    for step in 0..100u32 {
        let (x, y) = if step % 2 == 0 {
            (200.0, 120.0)
        } else {
            (10.0, 10.0)
        };
        doc.set_position_px("overlay", x, y).expect("move");
        let new = layout(&doc);
        let expected = union(outward(old.boxes[2].rect), outward(new.boxes[2].rect));
        assert_eq!(
            doc.take_incremental_damage(&old, &new, InteractionState::default(), viewport()),
            SurfaceDamage::Region(expected),
            "step {step}"
        );
        old = new;
    }
}

#[test]
fn large_overlay_promotes_to_full_at_threshold() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    // Old 50x40 at (10,10) union new 700x500 at (10,10) covers 350000px of 480000px viewport.
    doc.set_size_px("overlay", 700.0, 500.0).expect("resize");
    assert_eq!(damage(&mut doc, &old), SurfaceDamage::Full);
}

#[test]
fn region_just_under_threshold_stays_incremental() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    // 240000px exactly half of the viewport is not above the threshold.
    doc.set_size_px("overlay", 480.0, 490.0).expect("resize");
    assert!(matches!(damage(&mut doc, &old), SurfaceDamage::Region(_)));
}

#[test]
fn background_node_backdrop_forces_full() {
    let mut doc = scene(0.0);
    doc.set_node_backdrop("panel", Some(20)).expect("backdrop");
    let old = prime(&mut doc);
    doc.set_background_color("overlay", Color::BLACK)
        .expect("bg");
    assert_eq!(damage(&mut doc, &old), SurfaceDamage::Full);
}

#[test]
fn stale_layout_revision_forces_full() {
    let mut doc = scene(0.0);
    let old = prime(&mut doc);
    doc.set_background_color("overlay", Color::BLACK)
        .expect("bg");
    let stale = layout(&doc);
    doc.set_background_color("overlay", Color::WHITE)
        .expect("bg");
    assert_eq!(
        doc.take_incremental_damage(&old, &stale, InteractionState::default(), viewport()),
        SurfaceDamage::Full
    );
}
