//! Diagnostic timing for relayout, single-node style change and text change. Not a gate:
//! `cargo test --release -p flamewm-render-core --test layout_bench -- --ignored --nocapture`

use flamewm_render_compiler::{CompileOptions, compile_file};
use flamewm_render_core::text_layout::unscoped_measure_count;
use flamewm_render_core::{
    CompiledNode, InteractionState, LayoutEngine, NodeKind, RuntimeDocument,
};
use std::path::Path;
use std::time::Instant;

const OPS: u32 = 1000;

fn load(relative: &str) -> RuntimeDocument {
    let page = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    let compiled = compile_file(&page, &[], CompileOptions::default())
        .unwrap_or_else(|err| panic!("compile {relative}: {err}"));
    RuntimeDocument::new(compiled.document).expect("runtime document")
}

/// First element with an id whose only child is a single text node, and that text node's element id.
fn text_element_id(nodes: &[CompiledNode]) -> Option<String> {
    nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::Element
                && !node.id.is_empty()
                && node.first_child.is_some_and(|child| {
                    nodes[child as usize].kind == NodeKind::Text
                        && nodes[child as usize].next_sibling.is_none()
                })
        })
        .map(|node| node.id.to_string())
}

fn first_id(nodes: &[CompiledNode]) -> String {
    nodes
        .iter()
        .find(|node| node.kind == NodeKind::Element && !node.id.is_empty())
        .map(|node| node.id.to_string())
        .expect("an element with an id")
}

fn per_op(start: Instant) -> u128 {
    start.elapsed().as_nanos() / u128::from(OPS)
}

#[test]
#[ignore]
fn relayout_style_text_bench() {
    for page in ["ui/index.html", "ui/shell/dock.html", "ui/lock/lock.html"] {
        let mut doc = load(page);
        let interaction = InteractionState::default();
        let nodes_len = doc.document.nodes.len();
        let text_id = text_element_id(&doc.document.nodes);
        let style_id = first_id(&doc.document.nodes);

        // Same document with a UI font offset: every node resolves through the owned-style path.
        doc.set_ui_font_defaults(0.5, false)
            .expect("ui font defaults");
        let start = Instant::now();
        for _ in 0..OPS {
            std::hint::black_box(LayoutEngine::compute(&doc, 1280.0, 800.0, interaction));
        }
        let owned_style = per_op(start);
        doc.set_ui_font_defaults(0.0, false)
            .expect("ui font defaults");

        let measures_before = unscoped_measure_count();
        let start = Instant::now();
        for _ in 0..OPS {
            std::hint::black_box(LayoutEngine::compute(&doc, 1280.0, 800.0, interaction));
        }
        let relayout = per_op(start);
        let measures = (unscoped_measure_count() - measures_before) / u64::from(OPS);

        let start = Instant::now();
        for i in 0..OPS {
            let weight = if i % 2 == 0 { 700 } else { 400 };
            doc.set_font_weight(&style_id, weight).expect("weight");
            std::hint::black_box(LayoutEngine::compute(&doc, 1280.0, 800.0, interaction));
        }
        let style = per_op(start);

        let text = match text_id {
            Some(id) => {
                let start = Instant::now();
                for i in 0..OPS {
                    let text = format!("edited text {i} with a varying length {}", i * 13);
                    doc.set_text(&id, text).expect("text");
                    std::hint::black_box(LayoutEngine::compute(&doc, 1280.0, 800.0, interaction));
                }
                per_op(start).to_string()
            }
            None => "n/a".to_string(),
        };

        println!(
            "layout_bench page={page} nodes={nodes_len} text_measures/relayout={measures} ns/op: relayout={relayout} owned_style_relayout={owned_style} \
             style_change+relayout={style} text_change+relayout={text} \
             profile={}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }
}
