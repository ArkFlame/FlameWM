use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{compile_file, encode, CompileOptions};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let css = root.join("ui/shell/flamewm.css");
    let sources = [
        "panel.html",
        "start.html",
        "task-menu.html",
        "media.html",
        "audio.html",
        "network.html",
        "calendar.html",
    ];
    let artifacts = [
        ("flamewm-panel.rwr", "panel.html"),
        ("flamewm-start.rwr", "start.html"),
        ("flamewm-task-menu.rwr", "task-menu.html"),
        ("flamewm-media.rwr", "media.html"),
        ("flamewm-audio.rwr", "audio.html"),
        ("flamewm-network.rwr", "network.html"),
        ("flamewm-calendar.rwr", "calendar.html"),
    ];

    println!("cargo:rerun-if-changed={}", css.display());
    for source in sources {
        println!(
            "cargo:rerun-if-changed={}",
            root.join("ui/shell").join(source).display()
        );
    }
    let shell_assets = root.join("ui/shell/assets");
    rerun_tree(&shell_assets.join("web/breeze"));
    rerun_tree(&shell_assets.join("branding"));

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged_ui = out_dir.join("shell");
    let staged_assets = staged_ui.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged shell assets");
    for source in sources {
        let path = root.join("ui/shell").join(source);
        fs::copy(&path, staged_ui.join(source)).expect("stage shell UI source");
    }
    fs::copy(&css, staged_ui.join("flamewm.css")).expect("stage shell CSS");
    // Keyed PPM refs are retired: stage true-alpha SVG/Breeze + branding
    // paths so compiled templates resolve local build inputs only.
    stage_tree(
        &shell_assets.join("web/breeze"),
        &staged_assets.join("breeze"),
    );
    stage_tree(
        &shell_assets.join("branding"),
        &staged_assets.join("branding"),
    );
    for (artifact, source) in artifacts {
        compile_to(&staged_ui.join(source), &out_dir.join(artifact));
    }
}

fn stage_tree(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create staged asset dir");
    for entry in fs::read_dir(source).expect("read staged asset dir") {
        let entry = entry.expect("read staged asset entry");
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, dest.join(entry.file_name())).expect("stage asset file");
        }
    }
}

fn compile_to(source: &Path, output: &Path) {
    let compiled = compile_file(source, &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("failed to compile {}: {error}", source.display()));
    let bytes = encode(&compiled.document).expect("compiled FlameWM UI must encode");
    fs::write(output, bytes).expect("write compiled FlameWM UI artifact");
}

fn rerun_tree(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files = fs::read_dir(dir)
        .expect("read tracked asset dir")
        .map(|entry| entry.expect("read tracked asset entry").path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
}
