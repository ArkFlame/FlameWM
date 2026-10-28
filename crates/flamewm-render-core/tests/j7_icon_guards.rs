//! J7 icon/alpha ratchet: symbolic images tint from semantic foreground.

use flamewm_render_core::{
    Color, ColorValue, CompiledDocument, CompiledNode, ImageAsset, ImageTreatment,
    InteractionState, LayoutEngine, LayoutResult, NodeKind, PaintCommand, RuntimeDocument, Style,
    build_paint_commands,
};

fn image_doc(treatment: ImageTreatment, color: ColorValue) -> RuntimeDocument {
    let style = Style {
        image_treatment: treatment,
        color,
        ..Style::default()
    };
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 1,
        root: 0,
        variables: Vec::new(),
        assets: vec![ImageAsset {
            source: "icon".to_string().into(),
            width: 1,
            height: 1,
            pixels: vec![200, 200, 200, 255].into(),
        }],
        nodes: vec![CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "icon".to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: Some(0),
            style: style.into(),
            hover_style: None,
            active_style: None,
        }],
    })
    .expect("fixture validates")
}

fn layout_doc(doc: &RuntimeDocument) -> LayoutResult {
    LayoutEngine::compute(doc, 8.0, 8.0, InteractionState::default())
}

fn image_command(commands: &[PaintCommand]) -> Option<(&ImageTreatment, &Option<Color>)> {
    commands.iter().find_map(|command| match command {
        PaintCommand::Image {
            treatment, tint, ..
        } => Some((treatment, tint)),
        _ => None,
    })
}

#[test]
fn original_images_carry_no_tint() {
    let doc = image_doc(
        ImageTreatment::Original,
        ColorValue::Literal(Color::rgb(10, 20, 30)),
    );
    let layout = layout_doc(&doc);
    let commands = build_paint_commands(&doc, &layout, InteractionState::default());
    let (treatment, tint) = image_command(&commands).expect("image command");
    assert_eq!(*treatment, ImageTreatment::Original);
    assert_eq!(*tint, None);
}

#[test]
fn symbolic_images_consume_semantic_foreground() {
    let doc = image_doc(
        ImageTreatment::SymbolicForeground,
        ColorValue::Literal(Color::rgb(10, 20, 30)),
    );
    let layout = layout_doc(&doc);
    let commands = build_paint_commands(&doc, &layout, InteractionState::default());
    let (treatment, tint) = image_command(&commands).expect("image command");
    assert_eq!(*treatment, ImageTreatment::SymbolicForeground);
    assert_eq!(*tint, Some(Color::rgb(10, 20, 30)));
}

#[test]
fn symbolic_tint_changes_with_foreground_not_asset_bytes() {
    let red = image_doc(
        ImageTreatment::SymbolicForeground,
        ColorValue::Literal(Color::rgb(200, 10, 10)),
    );
    let blue = image_doc(
        ImageTreatment::SymbolicForeground,
        ColorValue::Literal(Color::rgb(10, 10, 200)),
    );
    let red_layout = layout_doc(&red);
    let blue_layout = layout_doc(&blue);
    let red_cmds = build_paint_commands(&red, &red_layout, InteractionState::default());
    let blue_cmds = build_paint_commands(&blue, &blue_layout, InteractionState::default());
    let (_, red_tint) = image_command(&red_cmds).expect("red image");
    let (_, blue_tint) = image_command(&blue_cmds).expect("blue image");
    assert_eq!(*red_tint, Some(Color::rgb(200, 10, 10)));
    assert_eq!(*blue_tint, Some(Color::rgb(10, 10, 200)));
}
