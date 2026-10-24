    use super::*;
    use flamewm_ui_x11::test_support::RecordingUiDoc;

    fn press(x: f32, y: f32) -> RawPress {
        RawPress {
            x,
            y,
            button: 1,
            time_ms: 0,
        }
    }

    #[test]
    fn classification_is_inside_on_rect_and_outside_past_its_exclusive_edges() {
        let rects = [Rect::new(10, 10, 20, 20)];
        assert!(!press_outside_menus(&rects, 10.0, 10.0));
        assert!(!press_outside_menus(&rects, 29.4, 29.4));
        assert!(press_outside_menus(&rects, 30.0, 15.0));
        assert!(press_outside_menus(&rects, 15.0, 30.0));
        assert!(press_outside_menus(&rects, 9.0, 15.0));
    }

    #[test]
    fn press_inside_any_open_rect_is_not_outside() {
        let rects = [Rect::new(0, 0, 10, 10), Rect::new(50, 50, 10, 10)];
        assert!(!press_outside_menus(&rects, 55.0, 55.0));
        assert!(press_outside_menus(&rects, 30.0, 30.0));
    }

    #[test]
    fn arming_follows_open_menus_only() {
        assert!(!menus_open(&[]));
        assert!(menus_open(&[Rect::new(0, 0, 1, 1)]));
    }

    #[test]
    fn outside_press_closes_menus_and_clears_pending_delete() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = vec![Rect::new(10, 10, 20, 20)];
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = Some(MenuState {
            kind: MenuKind::Entry,
            anchor: Point::new(10, 10),
            output: OutputId::new("default"),
            selected_id: Some("image.png".into()),
        });
        close_menus_on_outside_press(
            &press(100.0, 100.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("close");
        assert!(rects.is_empty());
        assert!(
            menu.is_none(),
            "outside press must release the semantic menu, not just hide it"
        );
        assert!(pending.is_none());
        assert_eq!(doc.visible.get("desktop-menu"), Some(&false));
        assert_eq!(doc.visible.get("desktop-confirm"), Some(&false));
    }

    #[test]
    fn inside_press_leaves_menus_and_pending_delete_untouched() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = vec![Rect::new(10, 10, 20, 20)];
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = Some(MenuState {
            kind: MenuKind::ConvertWheel,
            anchor: Point::new(10, 10),
            output: OutputId::new("default"),
            selected_id: Some("image.png".into()),
        });
        close_menus_on_outside_press(
            &press(12.0, 12.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("inside");
        assert_eq!(rects, vec![Rect::new(10, 10, 20, 20)]);
        assert!(
            menu.is_some(),
            "press inside the wheel must preserve its action target"
        );
        assert!(pending.is_some());
        assert!(doc.visible.is_empty());
    }

    #[test]
    fn press_while_closed_is_a_noop() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = Vec::new();
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = None;
        close_menus_on_outside_press(
            &press(5.0, 5.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("closed");
        assert!(menu.is_none());
        assert!(pending.is_some());
        assert!(doc.visible.is_empty());
    }
