use super::*;

#[cfg(test)]
mod ui_scale_tests {
    use super::*;

    fn clickable_document() -> RuntimeDocument {
        let root = CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: Some(1),
            next_sibling: None,
            id: String::new(),
            action: String::new(),
            text: String::new(),
            image: None,
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        let button = CompiledNode {
            kind: NodeKind::Element,
            parent: Some(0),
            first_child: None,
            next_sibling: None,
            id: "button".to_string(),
            action: "activate".to_string(),
            text: String::new(),
            image: None,
            style: Style {
                width: Length::Px(40.0),
                height: Length::Px(20.0),
                background: ColorValue::Literal(Color::WHITE),
                ..Style::default()
            },
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![root, button],
        })
        .expect("fixture document validates")
    }

    fn labelled_document() -> RuntimeDocument {
        let node = |kind, parent: Option<u32>, first_child: Option<u32>, id: &str, text: &str| {
            CompiledNode {
                kind,
                parent,
                first_child,
                next_sibling: None,
                id: id.to_string(),
                action: String::new(),
                text: text.to_string(),
                image: None,
                style: Style::default(),
                hover_style: None,
                active_style: None,
            }
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                node(NodeKind::Element, None, Some(1), "", ""),
                node(NodeKind::Element, Some(0), Some(2), "label", ""),
                node(NodeKind::Text, Some(1), None, "", "12:00"),
            ],
        })
        .expect("fixture document validates")
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

    fn image_doc() -> RuntimeDocument {
        let asset = ImageAsset {
            source: "old".to_string(),
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        let image_node = CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "icon".to_string(),
            action: String::new(),
            text: String::new(),
            image: Some(0),
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        let text_node = CompiledNode {
            kind: NodeKind::Text,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "label".to_string(),
            action: String::new(),
            text: "hi".to_string(),
            image: None,
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: vec![asset],
            nodes: vec![image_node, text_node],
        })
        .expect("fixture document validates")
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
        assert_eq!(entry.rgba8, vec![1, 2, 3, 200]);
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
        let asset = ImageAsset {
            source: "shared".to_string(),
            width: 1,
            height: 1,
            pixels: vec![9, 9, 9, 255],
        };
        let mk = |id: &str| CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: id.to_string(),
            action: String::new(),
            text: String::new(),
            image: Some(0),
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: vec![asset],
            nodes: vec![mk("a"), mk("b")],
        })
        .expect("fixture validates")
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
        let rev = |cmds: &Vec<crate::paint::PaintCommand>| match cmds.iter().find_map(|c| match c {
            crate::paint::PaintCommand::Image { revision, .. } => Some(*revision),
            _ => None,
        }) {
            Some(v) => v,
            None => panic!("expected image command"),
        };
        assert_eq!(rev(&before), 0);
        assert_eq!(rev(&after), 1);
    }
}
