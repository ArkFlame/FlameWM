//! Guards the backend split: the lock core must stay renderer-neutral so a Wayland
//! adapter can drive the same loop. Only the surface loop module may name a backend.

use std::fs;
use std::path::{Path, PathBuf};

fn core_modules() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut modules: Vec<PathBuf> = fs::read_dir(&root)
        .expect("lock src directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    modules.sort();
    modules
}

/// The two modules allowed to name a concrete backend: the presentation loop
/// (`run.rs`) and the X11 session/display adapter (`x11.rs`).
const BACKEND_MODULES: [&str; 2] = ["run.rs", "x11.rs"];

#[test]
fn lock_core_stays_renderer_neutral() {
    let mut offenders = Vec::new();
    for module in core_modules() {
        let name = module
            .file_name()
            .and_then(|name| name.to_str())
            .expect("module name");
        if BACKEND_MODULES.contains(&name) {
            continue;
        }
        let source = fs::read_to_string(&module).expect("read module");
        // Only code, not comments: a doc comment may name a backend to explain the split.
        for (index, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if code.contains("flamewm_ui_x11") {
                offenders.push(format!("{name}:{}", index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "lock core modules must not depend on the X11 UI backend: {offenders:?}"
    );
}

#[test]
fn lock_core_uses_the_shared_document_crate() {
    let document = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("document.rs"),
    )
    .expect("read document.rs");
    assert!(
        document.contains("flamewm_ui_document"),
        "the lock document layer must use the backend-neutral document crate"
    );
}
