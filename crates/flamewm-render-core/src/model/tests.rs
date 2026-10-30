use super::*;

fn fixture_node(kind: NodeKind, id: &str, text: &str, image: Option<u16>) -> CompiledNode {
    CompiledNode {
        kind,
        parent: None,
        first_child: None,
        next_sibling: None,
        id: id.to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: text.to_string().into(),
        image,
        style: Style::default().into(),
        hover_style: None,
        active_style: None,
    }
}

fn image_asset(source: &str, width: u32, height: u32, pixels: Vec<u8>) -> ImageAsset {
    ImageAsset {
        source: source.to_string().into(),
        width,
        height,
        pixels: pixels.into(),
    }
}

fn fixture_document(nodes: Vec<CompiledNode>, assets: Vec<ImageAsset>) -> RuntimeDocument {
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets,
        nodes,
    })
    .expect("fixture document validates")
}

#[cfg(test)]
mod paint_revision_tests {
    use super::*;

    #[test]
    fn paint_only_color_edits_keep_layout_revision_and_full_damage() {
        let mut doc = fixture_document(
            vec![fixture_node(NodeKind::Element, "box", "", None)],
            vec![],
        );
        let _ = doc.take_damage();
        let (layout_rev, rev) = (doc.layout_revision(), doc.revision());
        for step in 0..50u32 {
            let color = if step % 2 == 0 {
                Color::BLACK
            } else {
                Color::TRANSPARENT
            };
            doc.set_background_color("box", color).expect("background");
        }
        assert_eq!(
            doc.layout_revision(),
            layout_rev,
            "color edits must not relayout"
        );
        assert_eq!(
            doc.revision(),
            rev + 50,
            "every color change still repaints"
        );
        assert_eq!(
            doc.peek_damage(),
            SurfaceDamage::Full,
            "damage must not be lost"
        );
        let _ = doc.take_damage();
        doc.set_background_color("box", Color::TRANSPARENT)
            .expect("background");
        assert_eq!(doc.revision(), rev + 50, "same-value color is a no-op");
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn boundary_scroll_does_not_invalidate_or_allocate_retained_offset() {
        let mut doc = fixture_document(
            vec![fixture_node(NodeKind::Element, "scroll", "", None)],
            vec![],
        );
        let _ = doc.take_damage();
        let revision = doc.revision();
        let layout_revision = doc.layout_revision();
        // No scrollable extent: wheel inputs must neither repaint nor grow the map.
        for _ in 0..100 {
            let result = doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 48.0 }, (0.0, 0.0));
            assert_eq!(result, ScrollState::default());
        }
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.layout_revision(), layout_revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
        assert!(doc.scroll_offsets.is_empty());

        // Real scrolling still invalidates and the final overscroll is a no-op.
        let result = doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 24.0 }, (0.0, 48.0));
        assert_eq!(result.offset_y, 24.0);
        assert_eq!(doc.revision(), revision + 1);
        assert_eq!(doc.peek_damage(), SurfaceDamage::Full);
        let _ = doc.take_damage();
        assert_eq!(
            doc.apply_scroll_delta(
                0,
                ScrollDelta {
                    dx: 0.0,
                    dy: 1000.0
                },
                (0.0, 48.0)
            )
            .offset_y,
            48.0
        );
        let final_revision = doc.revision();
        let _ = doc.take_damage();
        for _ in 0..100 {
            assert_eq!(
                doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 5.0 }, (0.0, 48.0))
                    .offset_y,
                48.0
            );
        }
        assert_eq!(doc.revision(), final_revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
        // Clamping after content shrink still invalidates if the offset changes.
        assert_eq!(
            doc.apply_scroll_delta(0, ScrollDelta { dx: 0.0, dy: 0.0 }, (0.0, 10.0))
                .offset_y,
            10.0
        );
        assert_eq!(doc.revision(), final_revision + 1);
    }

    #[test]
    fn geometry_and_text_edits_still_bump_layout_revision() {
        let mut doc = fixture_document(
            vec![fixture_node(NodeKind::Element, "box", "", None)],
            vec![],
        );
        let layout_rev = doc.layout_revision();
        doc.set_size_px("box", 33.0, 17.0).expect("size");
        assert_eq!(doc.layout_revision(), layout_rev + 1);
        doc.set_border_color("box", Color::BLACK).expect("border");
        assert_eq!(
            doc.layout_revision(),
            layout_rev + 1,
            "border color is paint-only"
        );
    }
}

#[cfg(test)]
mod ui_scale_tests {
    use super::*;

    fn clickable_document() -> RuntimeDocument {
        fixture_document(
            vec![
                CompiledNode {
                    first_child: Some(1),
                    ..fixture_node(NodeKind::Element, "", "", None)
                },
                CompiledNode {
                    parent: Some(0),
                    action: "activate".to_string().into(),
                    style: Style {
                        width: Length::Px(40.0),
                        height: Length::Px(20.0),
                        background: ColorValue::Literal(Color::WHITE),
                        ..Style::default()
                    }
                    .into(),
                    ..fixture_node(NodeKind::Element, "button", "", None)
                },
            ],
            Vec::new(),
        )
    }

    fn labelled_document() -> RuntimeDocument {
        fixture_document(
            vec![
                CompiledNode {
                    first_child: Some(1),
                    ..fixture_node(NodeKind::Element, "", "", None)
                },
                CompiledNode {
                    parent: Some(0),
                    first_child: Some(2),
                    ..fixture_node(NodeKind::Element, "label", "", None)
                },
                CompiledNode {
                    parent: Some(1),
                    ..fixture_node(NodeKind::Text, "", "12:00", None)
                },
            ],
            Vec::new(),
        )
    }

    #[test]
    fn runtime_foreground_and_weight_reach_the_text_children_of_the_element() {
        let mut document = labelled_document();
        document
            .set_foreground_color("label", ColorValue::Literal(Color::rgb(255, 0, 0)))
            .expect("foreground");
        document.set_font_weight("label", 700).expect("weight");
        let element = document.node_by_id("label").expect("label");
        let text = document.document.nodes[element as usize]
            .first_child
            .expect("text child");
        for index in [element, text] {
            let style = document.runtime_style(index, InteractionState::default());
            assert_eq!(
                style.color,
                ColorValue::Literal(Color::rgb(255, 0, 0)),
                "node {index} colour"
            );
            assert_eq!(style.font_weight, 700, "node {index} weight");
        }
    }

    #[test]
    fn runtime_font_size_reaches_the_text_children_of_the_element() {
        let mut document = labelled_document();
        document.set_font_size_px("label", 51.0).expect("font size");
        let element = document.node_by_id("label").expect("label");
        let text = document.document.nodes[element as usize]
            .first_child
            .expect("text child");
        for index in [element, text] {
            let style = document.runtime_style(index, InteractionState::default());
            assert_eq!(
                style.font_size, 51.0,
                "node {index} must carry the override"
            );
        }
    }

    #[test]
    fn global_font_defaults_apply_after_node_styles_and_invalidate_layout() {
        let mut document = labelled_document();
        let label = document.node_by_id("label").expect("label");
        let text = document.document.nodes[label as usize]
            .first_child
            .expect("text");
        let before = document.runtime_style(text, InteractionState::default());
        let revision = document.revision();
        let _ = document.take_damage();

        document
            .set_ui_font_defaults(2.0, true)
            .expect("global font defaults");

        assert_eq!(document.revision(), revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
        for index in [label, text] {
            let style = document.runtime_style(index, InteractionState::default());
            assert_eq!(style.font_size, before.font_size + 2.0);
            assert!(style.font_weight >= 600);
        }
        assert!(document.set_ui_font_defaults(f32::NAN, false).is_err());
    }

    #[test]
    fn bench_setter_revisions_and_damage() {
        let mut document = labelled_document();
        let _ = document.take_damage();
        let start = document.revision();
        document.set_text("label", "bench-0").expect("first text");
        let first_text_revs = document.revision() - start;
        let _ = document.take_damage();
        let before = document.revision();
        for _ in 0..10 {
            document
                .set_text("label", "bench-0")
                .expect("identical text");
        }
        let identical_text_revs = document.revision() - before;
        let identical_text_damage = document.take_damage();

        let mut clickable = clickable_document();
        clickable
            .set_background_color("button", Color::rgb(1, 2, 3))
            .expect("first paint");
        let _ = clickable.take_damage();
        let before = clickable.revision();
        for _ in 0..10 {
            clickable
                .set_background_color("button", Color::rgb(1, 2, 3))
                .expect("identical paint");
        }
        let identical_paint_revs = clickable.revision() - before;
        let identical_paint_damage = clickable.take_damage();
        clickable
            .set_background_color("button", Color::rgb(9, 9, 9))
            .expect("paint-only change");
        let paint_revs = clickable.revision() - before;
        let paint_damage = clickable.peek_damage();
        eprintln!(
            "BENCH render_first_text_revisions={first_text_revs} render_identical_text_x10_revisions={identical_text_revs} \
             render_identical_text_x10_damage={identical_text_damage:?} render_identical_paint_x10_revisions={identical_paint_revs} \
             render_identical_paint_x10_damage={identical_paint_damage:?} render_paint_only_change_revisions={paint_revs} \
             render_paint_only_change_damage={paint_damage:?}"
        );
        assert_eq!(first_text_revs, 1);
        assert_eq!(identical_text_revs, 0);
        assert_eq!(identical_text_damage, SurfaceDamage::None);
        assert_eq!(identical_paint_revs, 0);
        assert_eq!(identical_paint_damage, SurfaceDamage::None);
        assert_eq!(paint_revs, 1);
        assert_eq!(paint_damage, SurfaceDamage::Full);
    }

    #[test]
    fn color_override_mutation_invalidates_paint_and_applies_to_runtime_style() {
        let mut document = clickable_document();
        let button = document.node_by_id("button").expect("button node");
        let revision = document.revision();
        let _ = document.take_damage();

        document
            .set_background_color("button", Color::rgb(255, 0, 0))
            .expect("background override");
        assert_eq!(document.revision(), revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
        let style = document.runtime_style(button, InteractionState::default());
        assert_eq!(
            document.resolve_color(style.background),
            Color::rgb(255, 0, 0)
        );

        let revision = document.revision();
        let _ = document.take_damage();
        document
            .clear_background_color("button")
            .expect("background clear");
        assert_eq!(document.revision(), revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
    }

    #[test]
    fn ui_scale_mutation_invalidates_retained_layout_and_paint_once() {
        let mut document = clickable_document();
        let revision = document.revision();
        let _ = document.take_damage();

        document.set_ui_scale(1.5).expect("valid scale");
        assert_eq!(document.ui_scale(), 1.5);
        assert_eq!(document.revision(), revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);

        let revision = document.revision();
        let _ = document.take_damage();
        document.set_ui_scale(1.5).expect("same scale is valid");
        assert_eq!(document.revision(), revision);
        assert_eq!(document.peek_damage(), SurfaceDamage::None);

        // Layout and paint retain logical coordinates. Viewport and pointer
        // coordinates cross the device boundary using this document scale.
        let scale = document.ui_scale();
        let layout = crate::layout::LayoutEngine::compute(
            &document,
            100.0 / scale,
            100.0 / scale,
            InteractionState::default(),
        );
        assert_eq!(layout.revision, document.revision());
        assert_eq!(layout.boxes[1].rect.width, 40.0);
        assert_eq!(layout.boxes[1].rect.height, 20.0);
        assert_eq!(
            layout.hit_test_action(&document, 58.5 / scale, 28.5 / scale),
            Some(1)
        );
        assert_eq!(
            layout.hit_test_action(&document, 60.0 / scale, 15.0 / scale),
            None
        );
        let paint =
            crate::paint::build_paint_commands(&document, &layout, InteractionState::default());
        let button_paint = crate::paint::PaintCommand::FillRect {
            rect: layout.boxes[1].rect,
            color: Color::WHITE,
            radius: 0.0,
        };
        assert!(paint.contains(&button_paint));
        let crate::paint::PaintCommand::FillRect { rect, .. } = button_paint else {
            unreachable!("button paint is a fill rect")
        };
        assert_eq!(rect.width * scale, 60.0);
        assert_eq!(rect.height * scale, 30.0);
    }

    #[test]
    fn hidden_mode_like_action_is_not_painted_or_hit_testable() {
        let mut document = clickable_document();
        document
            .set_visible("button", false)
            .expect("hide inactive slot");
        let layout = crate::layout::LayoutEngine::compute(
            &document,
            100.0,
            100.0,
            InteractionState::default(),
        );
        assert_eq!(layout.hit_test_action(&document, 10.0, 10.0), None);
        assert!(crate::paint::build_paint_commands(
            &document,
            &layout,
            InteractionState::default(),
        )
        .iter()
        .all(|command| !matches!(command,
            crate::paint::PaintCommand::FillRect { rect, .. } if *rect == layout.boxes[1].rect
        )));
    }

    #[test]
    fn ui_scale_rejects_non_finite_and_non_positive_values() {
        let mut document = clickable_document();
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(document.set_ui_scale(scale).is_err());
            assert_eq!(document.ui_scale(), 1.0);
        }
    }
}

#[cfg(test)]
mod image_replace_tests {
    use super::*;
    use std::sync::Arc;

    fn image_doc() -> RuntimeDocument {
        fixture_document(
            vec![
                fixture_node(NodeKind::Image, "icon", "", Some(0)),
                fixture_node(NodeKind::Text, "label", "hi", None),
            ],
            vec![image_asset(
                "old",
                2,
                1,
                vec![10, 20, 30, 255, 40, 50, 60, 255],
            )],
        )
    }

    #[test]
    fn success_replaces_and_bumps_revision() {
        let mut doc = image_doc();
        let before = doc.document.assets[0].clone();
        doc.replace_image_for_node("icon", "new".to_string(), 1, 1, vec![1, 2, 3, 200])
            .expect("replace succeeds");
        // Compiled assets stay immutable; override is node-local.
        assert_eq!(doc.document.assets[0], before);
        let entry = doc.node_image_override(0).expect("override stored");
        assert_eq!(entry.source, "new");
        assert_eq!((entry.width, entry.height), (1, 1));
        assert_eq!(*entry.rgba8, vec![1, 2, 3, 200]);
        assert_eq!(doc.image_revision_for_node(0), 1);
        assert_eq!(doc.image_revision(0), 0);
    }

    #[test]
    fn second_replace_bumps_again() {
        let mut doc = image_doc();
        doc.replace_image_for_node("icon", "a".to_string(), 1, 1, vec![1, 2, 3, 255])
            .expect("first replace");
        doc.replace_image_for_node("icon", "b".to_string(), 2, 1, vec![4u8; 8])
            .expect("second replace");
        let entry = doc.node_image_override(0).expect("override stored");
        assert_eq!(entry.source, "b");
        assert_eq!(doc.image_revision_for_node(0), 2);
        assert_eq!(doc.document.assets[0].source, "old");
    }

    #[test]
    fn equal_shared_replace_adopts_caller_buffer_without_bumping() {
        let mut doc = image_doc();
        let first = Arc::new(vec![1, 2, 3, 255]);
        doc.replace_image_for_node_shared("icon", "a".to_string(), 1, 1, Arc::clone(&first))
            .expect("first replace");
        let second = Arc::new(vec![1, 2, 3, 255]);
        doc.replace_image_for_node_shared("icon", "a".to_string(), 1, 1, Arc::clone(&second))
            .expect("equal replace");
        let entry = doc.node_image_override(0).expect("override stored");
        assert!(Arc::ptr_eq(&entry.rgba8, &second));
        assert_eq!(Arc::strong_count(&first), 1);
        assert_eq!(doc.image_revision_for_node(0), 1);
    }

    #[test]
    fn wrong_kind_errs() {
        let mut doc = image_doc();
        let err = doc
            .replace_image_for_node("label", "x".to_string(), 1, 1, vec![0, 0, 0, 255])
            .expect_err("text node must fail");
        assert!(err.contains("not an image node"), "unexpected: {err}");
        assert_eq!(doc.image_revision_for_node(1), 0);
    }

    #[test]
    fn zero_dims_err() {
        let mut doc = image_doc();
        assert!(
            doc.replace_image_for_node("icon", "x".to_string(), 0, 1, vec![0, 0, 0, 255])
                .is_err()
        );
        assert!(
            doc.replace_image_for_node("icon", "x".to_string(), 1, 0, Vec::new())
                .is_err()
        );
        assert!(doc.node_image_override(0).is_none());
    }

    #[test]
    fn byte_count_mismatch_errs() {
        let mut doc = image_doc();
        let err = doc
            .replace_image_for_node("icon", "x".to_string(), 2, 2, vec![0u8; 8])
            .expect_err("must fail");
        assert!(err.contains("expected 16"), "unexpected: {err}");
        assert!(doc.node_image_override(0).is_none());
    }

    #[test]
    fn unknown_id_errs() {
        let mut doc = image_doc();
        assert!(
            doc.replace_image_for_node("missing", "x".to_string(), 1, 1, vec![0, 0, 0, 255])
                .is_err()
        );
    }

    #[test]
    fn opaque_rgba_fixture_reports_fully_opaque() {
        let doc = image_doc();
        assert!(doc.document.assets[0].is_fully_opaque());
    }

    fn shared_asset_doc() -> RuntimeDocument {
        fixture_document(
            vec![
                fixture_node(NodeKind::Image, "a", "", Some(0)),
                fixture_node(NodeKind::Image, "b", "", Some(0)),
            ],
            vec![image_asset("shared", 1, 1, vec![9, 9, 9, 255])],
        )
    }

    #[test]
    fn shared_asset_isolation() {
        let mut doc = shared_asset_doc();
        doc.replace_image_for_node("a", "solo".to_string(), 1, 1, vec![1, 2, 3, 255])
            .expect("replace a");
        assert_eq!(doc.document.assets[0].pixels, vec![9, 9, 9, 255]);
        assert!(doc.node_image_override(1).is_none());
        let (_, _, _, _, pixels) = doc.image_for_node(1).expect("b intact");
        assert_eq!(pixels, &[9, 9, 9, 255]);
        assert_eq!(doc.image_revision_for_node(0), 1);
        assert_eq!(doc.image_revision_for_node(1), 0);
    }

    #[test]
    fn repeated_same_node_replace_stays_bounded() {
        let mut doc = image_doc();
        for i in 0..100u8 {
            doc.replace_image_for_node("icon", format!("f{i}"), 1, 1, vec![i, i, i, 255])
                .expect("replace");
        }
        let entry = doc.node_image_override(0).expect("override");
        assert_eq!(entry.rgba8.len(), 4);
        assert_eq!(doc.image_revision_for_node(0), 100);
        assert_eq!(doc.document.assets[0].pixels.len(), 8);
    }

    #[test]
    fn paint_identity_tracks_node_revision() {
        let mut doc = image_doc();
        let layout =
            crate::layout::LayoutEngine::compute(&doc, 50.0, 50.0, InteractionState::default());
        let before = crate::paint::build_paint_commands(&doc, &layout, InteractionState::default());
        doc.replace_image_for_node("icon", "n".to_string(), 1, 1, vec![5, 6, 7, 255])
            .expect("replace");
        let layout2 =
            crate::layout::LayoutEngine::compute(&doc, 50.0, 50.0, InteractionState::default());
        let after = crate::paint::build_paint_commands(&doc, &layout2, InteractionState::default());
        let rev = |cmds: &Vec<crate::paint::PaintCommand>| {
            cmds.iter()
                .find_map(|c| match c {
                    crate::paint::PaintCommand::Image { revision, .. } => Some(*revision),
                    _ => None,
                })
                .expect("expected image command")
        };
        assert_eq!(rev(&before), 0);
        assert_eq!(rev(&after), 1);
    }
}

#[cfg(test)]
mod idempotent_mutation_tests {
    use super::*;

    fn fixture() -> RuntimeDocument {
        fixture_document(
            vec![
                CompiledNode {
                    first_child: Some(1),
                    ..fixture_node(NodeKind::Element, "", "", None)
                },
                CompiledNode {
                    first_child: Some(2),
                    ..fixture_node(NodeKind::Element, "label", "", None)
                },
                fixture_node(NodeKind::Text, "", "12:00", None),
                fixture_node(NodeKind::Image, "icon", "", Some(0)),
            ],
            vec![image_asset("old", 1, 1, vec![1, 2, 3, 255])],
        )
    }

    #[test]
    fn set_text_same_value_keeps_revision_and_damage() {
        let mut doc = fixture();
        let revision = doc.revision();
        let _ = doc.take_damage();
        doc.set_text("label", "12:00").expect("same text");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn set_text_changed_value_bumps_revision() {
        let mut doc = fixture();
        let revision = doc.revision();
        let _ = doc.take_damage();
        doc.set_text("label", "13:00").expect("new text");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        assert_eq!(doc.peek_damage(), SurfaceDamage::Full);
        let revision = doc.revision();
        let _ = doc.take_damage();
        doc.set_text("label", "13:00").expect("same override");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn style_setters_same_value_keep_revision_and_damage() {
        let mut doc = fixture();
        let red = Color::rgb(255, 0, 0);
        doc.set_background_color("label", red).expect("first");
        doc.set_border_color("label", red).expect("first");
        doc.set_node_backdrop("label", Some(20)).expect("first");
        doc.set_border_radius("label", 4.0).expect("first");
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_background_color("label", red).expect("same");
        doc.set_border_color("label", red).expect("same");
        doc.set_node_backdrop("label", Some(20)).expect("same");
        doc.set_border_radius("label", 4.0).expect("same");
        doc.clear_foreground_color("label").expect("already unset");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);

        doc.clear_background_color("label").expect("clear");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        doc.clear_background_color("label").expect("already clear");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
    }

    #[test]
    fn text_style_setters_bump_revision_only_on_change() {
        let mut doc = fixture();
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_font_size_px("label", 14.0).expect("size");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        assert_eq!(doc.peek_damage(), SurfaceDamage::Full);
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_font_size_px("label", 14.0).expect("same size");
        doc.set_font_weight("label", 700).expect("weight");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_font_weight("label", 700).expect("same weight");
        doc.set_overflow("label", Overflow::Hidden, Overflow::Visible)
            .expect("overflow");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_overflow("label", Overflow::Hidden, Overflow::Visible)
            .expect("same overflow");
        let red = ColorValue::Literal(Color::rgb(255, 0, 0));
        doc.set_foreground_color("label", red).expect("color");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        let _ = doc.take_damage();
        let revision = doc.revision();

        doc.set_foreground_color("label", red).expect("same color");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn set_visible_same_state_keeps_revision_and_damage() {
        let mut doc = fixture();
        let revision = doc.revision();
        let _ = doc.take_damage();
        doc.set_visible("label", true).expect("already visible");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
        doc.set_visible("label", false).expect("hide");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
        let revision = doc.revision();
        let _ = doc.take_damage();
        doc.set_visible("label", false).expect("already hidden");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn replace_image_same_pixels_keeps_revision_and_damage() {
        let mut doc = fixture();
        doc.replace_image_for_node("icon", "n".to_string(), 1, 1, vec![5, 6, 7, 255])
            .expect("first replace");
        let revision = doc.revision();
        let image_revision = doc.image_revision_for_node(3);
        let _ = doc.take_damage();
        doc.replace_image_for_node("icon", "n".to_string(), 1, 1, vec![5, 6, 7, 255])
            .expect("same image");
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.image_revision_for_node(3), image_revision);
        assert_eq!(doc.peek_damage(), SurfaceDamage::None);
        doc.replace_image_for_node("icon", "n".to_string(), 1, 1, vec![9, 9, 9, 255])
            .expect("changed pixels");
        assert_eq!(doc.revision(), revision.wrapping_add(1));
    }
}

#[cfg(test)]
mod geometry_mutation_tests {
    use super::*;

    fn drag_ghost_document() -> RuntimeDocument {
        fixture_document(
            vec![
                CompiledNode {
                    first_child: Some(1),
                    style: Style {
                        width: Length::Px(800.0),
                        height: Length::Px(600.0),
                        ..Style::default()
                    }
                    .into(),
                    ..fixture_node(NodeKind::Element, "desktop", "", None)
                },
                CompiledNode {
                    parent: Some(0),
                    style: Style {
                        position: Position::Absolute,
                        width: Length::Px(82.0),
                        height: Length::Px(82.0),
                        ..Style::default()
                    }
                    .into(),
                    ..fixture_node(NodeKind::Element, "desktop-drag-ghost", "", None)
                },
            ],
            Vec::new(),
        )
    }

    #[test]
    fn pointer_motion_repositions_drag_ghost_and_invalidates_retained_frame() {
        let mut document = drag_ghost_document();
        let index = document.node_by_id("desktop-drag-ghost").unwrap() as usize;
        let _ = document.take_damage();
        let initial_revision = document.revision();

        document
            .set_position_px("desktop-drag-ghost", 80.0, 120.0)
            .unwrap();
        assert_eq!(document.revision(), initial_revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
        let first = crate::layout::LayoutEngine::compute(
            &document,
            800.0,
            600.0,
            InteractionState::default(),
        );
        assert_eq!(first.revision, document.revision());
        assert_eq!(
            (first.boxes[index].rect.x, first.boxes[index].rect.y),
            (80.0, 120.0)
        );

        let _ = document.take_damage();
        let first_revision = document.revision();
        document
            .set_position_px("desktop-drag-ghost", 125.0, 165.0)
            .unwrap();
        assert_eq!(document.revision(), first_revision.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
        let second = crate::layout::LayoutEngine::compute(
            &document,
            800.0,
            600.0,
            InteractionState::default(),
        );
        assert_eq!(
            (second.boxes[index].rect.x, second.boxes[index].rect.y),
            (125.0, 165.0)
        );

        let _ = document.take_damage();
        let stable_revision = document.revision();
        document
            .set_position_px("desktop-drag-ghost", 125.0, 165.0)
            .unwrap();
        assert_eq!(document.revision(), stable_revision);
        assert_eq!(document.peek_damage(), SurfaceDamage::None);
    }

    #[test]
    fn size_mutation_invalidates_layout_and_repeated_size_is_noop() {
        let mut document = drag_ghost_document();
        let _ = document.take_damage();
        let previous = document.revision();
        document
            .set_size_px("desktop-drag-ghost", 88.0, 90.0)
            .unwrap();
        assert_eq!(document.revision(), previous.wrapping_add(1));
        assert_eq!(document.peek_damage(), SurfaceDamage::Full);
        let index = document.node_by_id("desktop-drag-ghost").unwrap() as usize;
        let layout = crate::layout::LayoutEngine::compute(
            &document,
            800.0,
            600.0,
            InteractionState::default(),
        );
        assert_eq!(
            (
                layout.boxes[index].rect.width,
                layout.boxes[index].rect.height
            ),
            (88.0, 90.0)
        );
        let _ = document.take_damage();
        let revision = document.revision();
        document
            .set_size_px("desktop-drag-ghost", 88.0, 90.0)
            .unwrap();
        assert_eq!(document.revision(), revision);
        assert_eq!(document.peek_damage(), SurfaceDamage::None);
        assert!(
            document
                .set_position_px("desktop-drag-ghost", f32::NAN, 0.0)
                .is_err()
        );
        assert!(
            document
                .set_size_px("desktop-drag-ghost", -1.0, 0.0)
                .is_err()
        );
        assert_eq!(document.revision(), revision);
    }
}

#[cfg(test)]
mod lookup_index_tests {
    use super::*;

    #[test]
    fn node_by_id_finds_every_named_node_and_rejects_unknown_ids() {
        let nodes = ["zeta", "alpha", "", "mid"]
            .iter()
            .map(|id| fixture_node(NodeKind::Element, id, "", None))
            .collect();
        let document = fixture_document(nodes, Vec::new());
        assert_eq!(document.node_by_id("zeta"), Some(0));
        assert_eq!(document.node_by_id("alpha"), Some(1));
        assert_eq!(document.node_by_id("mid"), Some(3));
        assert_eq!(document.node_by_id(""), None);
        assert_eq!(document.node_by_id("missing"), None);
    }

    #[test]
    fn duplicate_runtime_ids_are_rejected() {
        let document = CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                fixture_node(NodeKind::Element, "a", "", None),
                fixture_node(NodeKind::Element, "b", "", None),
                fixture_node(NodeKind::Element, "a", "", None),
            ],
        };
        assert_eq!(
            RuntimeDocument::new(document).unwrap_err(),
            "duplicate runtime id 'a'"
        );
    }

    #[test]
    fn duplicate_variable_names_resolve_to_last_definition() {
        let variable = |name: &str| ColorVariable {
            name: name.to_string().into(),
            default: Color::BLACK,
        };
        let document = CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: vec![variable("--x"), variable("--y"), variable("--x")],
            assets: Vec::new(),
            nodes: vec![fixture_node(NodeKind::Element, "root", "", None)],
        };
        let mut runtime = RuntimeDocument::new(document).unwrap();
        assert_eq!(runtime.variable_by_name("--x"), Some(2));
        assert_eq!(runtime.variable_by_name("--y"), Some(1));
        assert_eq!(runtime.variable_by_name("--z"), None);
        assert_eq!(runtime.variable_by_name(""), None);
        runtime
            .set_color_variable("--x", Color::TRANSPARENT)
            .unwrap();
        assert_eq!(runtime.variable_values[2], Color::TRANSPARENT);
        assert_eq!(runtime.variable_values[0], Color::BLACK);
    }
}

#[cfg(test)]
mod z_index_tests {
    use super::*;

    fn chain_document() -> RuntimeDocument {
        let link =
            |parent: Option<u32>, first_child: Option<u32>, next_sibling: Option<u32>, id: &str| {
                CompiledNode {
                    parent,
                    first_child,
                    next_sibling,
                    ..fixture_node(NodeKind::Element, id, "", None)
                }
            };
        fixture_document(
            vec![
                link(None, Some(1), None, "root"),
                link(Some(0), Some(3), Some(2), "a"),
                link(Some(0), Some(4), None, "b"),
                link(Some(1), None, None, "a-child"),
                link(Some(2), Some(5), None, "b-child"),
                link(Some(4), None, None, "b-grandchild"),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn bulk_z_indices_match_the_per_node_walk() {
        let mut document = chain_document();
        assert_eq!(document.effective_z_indices(), None);
        document.set_z_index("b", 7).unwrap();
        document.set_z_index("b-grandchild", -2).unwrap();
        document.set_z_index("a-child", 3).unwrap();
        let bulk = document.effective_z_indices().expect("overrides present");
        let walked: Vec<i32> = (0..6)
            .map(|index| document.effective_z_index(index))
            .collect();
        assert_eq!(bulk, walked);
        assert_eq!(bulk, vec![0, 0, 7, 3, 7, -2]);
    }
}
