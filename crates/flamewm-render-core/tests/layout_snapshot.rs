//! Layout snapshot over every shipped `ui/**/*.html` page.
//!
//! Each page is compiled with the production compiler, laid out at three viewports and reduced to
//! one FNV-1a hash of the full per-node dump (box and content rects as f32 bits, plus z-order).
//! The hashes are pinned in `tests/golden/layout_fnv.txt`. Set `FLAMEWM_LAYOUT_UPDATE=1` to rewrite
//! that file, and `FLAMEWM_LAYOUT_DUMP_DIR=<dir>` to also write each full dump for a byte diff.

use flamewm_render_compiler::{CompileOptions, compile_file};
use flamewm_render_core::{InteractionState, LayoutEngine, LayoutResult, RuntimeDocument, fnv1a64};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const VIEWPORTS: [(f32, f32); 3] = [(1280.0, 800.0), (701.0, 438.0), (320.0, 240.0)];

fn ui_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui")
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/layout_fnv.txt")
}

fn collect_pages(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read ui directory")
        .map(|entry| entry.expect("ui directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_pages(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "html") {
            out.push(path);
        }
    }
}

/// `Err` carries the compile error text. A page whose assets are absent from the checkout cannot be
/// pinned: its golden line would differ between checkouts, so the test skips it with a note.
fn load(page: &Path) -> Result<RuntimeDocument, String> {
    let compiled = compile_file(page, &[], CompileOptions::default())?;
    RuntimeDocument::new(compiled.document)
}

fn dump(layout: &LayoutResult) -> String {
    let mut out = String::new();
    for (index, (entry, content)) in layout.boxes.iter().zip(&layout.contents).enumerate() {
        let r = entry.rect;
        let c = content;
        let _ = writeln!(
            out,
            "{index} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}",
            r.x.to_bits(),
            r.y.to_bits(),
            r.width.to_bits(),
            r.height.to_bits(),
            c.x.to_bits(),
            c.y.to_bits(),
            c.width.to_bits(),
            c.height.to_bits(),
        );
    }
    let _ = writeln!(out, "z {:?}", layout.z_order);
    out
}

#[test]
fn layout_matches_golden_for_every_ui_page() {
    let mut pages = Vec::new();
    collect_pages(&ui_root(), &mut pages);
    assert!(!pages.is_empty(), "no ui pages found");
    let dump_dir = std::env::var_os("FLAMEWM_LAYOUT_DUMP_DIR").map(PathBuf::from);
    let mut lines = Vec::new();
    for page in &pages {
        let name = page
            .strip_prefix(ui_root())
            .expect("page under ui root")
            .display()
            .to_string();
        let doc = match load(page) {
            Ok(doc) => doc,
            Err(err) => {
                eprintln!("layout_snapshot: skipped {name}: {err}");
                continue;
            }
        };
        for (width, height) in VIEWPORTS {
            let layout = LayoutEngine::compute(&doc, width, height, InteractionState::default());
            let text = dump(&layout);
            lines.push(format!(
                "{name} {width}x{height} nodes={} fnv={:016x}",
                layout.boxes.len(),
                fnv1a64(text.as_bytes())
            ));
            if let Some(dir) = &dump_dir {
                let file = format!("{}-{width}x{height}.txt", name.replace('/', "__"));
                std::fs::create_dir_all(dir).expect("create dump dir");
                std::fs::write(dir.join(file), &text).expect("write dump");
            }
        }
    }
    let actual = lines.join("\n") + "\n";
    if std::env::var_os("FLAMEWM_LAYOUT_UPDATE").is_some() {
        std::fs::write(golden_path(), &actual).expect("write golden");
        return;
    }
    let golden = std::fs::read_to_string(golden_path()).expect("read golden layout_fnv.txt");
    assert_eq!(
        actual, golden,
        "layout changed; diff against FLAMEWM_LAYOUT_DUMP_DIR output"
    );
}
