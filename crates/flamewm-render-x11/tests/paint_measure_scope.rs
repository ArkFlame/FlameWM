//! One real Xft paint must measure no text outside a scoped backend measure.
//! Run with a live display: `DISPLAY=:99 cargo test -p flamewm-render-x11 --test paint_measure_scope -- --ignored`.

use std::borrow::Cow;

use flamewm_render_core::text_layout::unscoped_measure_count;
use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};
use flamewm_render_x11::{SurfaceConfig, SurfaceController, SurfaceInputMode, SurfaceRole};

fn node(kind: NodeKind, parent: Option<u32>, first_child: Option<u32>, text: &'static str) -> CompiledNode {
    CompiledNode {
        kind,
        parent,
        first_child,
        next_sibling: None,
        id: Cow::Borrowed(""),
        action: Cow::Borrowed(""),
        text: Cow::Borrowed(text),
        image: None,
        style: Style::default().into(),
        hover_style: None,
        active_style: None,
        focus_style: None,
    }
}

fn text_document() -> RuntimeDocument {
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            node(NodeKind::Element, None, Some(1), ""),
            node(NodeKind::Text, Some(0), None, "Search query 1234567890"),
        ],
    })
    .expect("text fixture validates")
}

#[test]
#[ignore = "needs a live X display (DISPLAY)"]
fn real_xft_paint_measures_no_text_unscoped() {
    assert!(std::env::var_os("DISPLAY").is_some(), "DISPLAY must name a live X display");
    let mut controller = SurfaceController::new().expect("open controller");
    let before = unscoped_measure_count();
    let id = controller
        .create_surface(
            text_document(),
            SurfaceConfig {
                width: 400,
                height: 80,
                title: "measure-scope-probe".to_string(),
                role: SurfaceRole::Normal,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .expect("create surface");
    controller.show(id).expect("paint surface");
    assert_eq!(unscoped_measure_count(), before, "paint measured text outside the Xft scope");
}
