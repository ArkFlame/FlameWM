use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{compile_file, encode, rerun_tree, stage_tree, CompileOptions};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let css = root.join("ui/shell/flamewm.css");
    let menu_css = root.join("ui/shared/menu.css");
    let sources = [
        "panel.html",
        "panel-vertical.html",
        "dock.html",
        "dock-menu.html",
        "dock-label.html",
        "start.html",
        "task-menu.html",
        "media.html",
        "audio.html",
        "network.html",
        "calendar.html",
        "osd.html",
        "launch-feedback.html",
        "battery.html",
        "weather.html",
        "notification.html",
        "notification-center.html",
        "launcher.html",
        "switcher.html",
        "polkit.html",
    ];
    let artifacts = [
        ("flamewm-panel.rwr", "panel.html"),
        ("flamewm-panel-vertical.rwr", "panel-vertical.html"),
        ("flamewm-dock.rwr", "dock.html"),
        ("flamewm-dock-menu.rwr", "dock-menu.html"),
        ("flamewm-dock-label.rwr", "dock-label.html"),
        ("flamewm-start.rwr", "start.html"),
        ("flamewm-task-menu.rwr", "task-menu.html"),
        ("flamewm-media.rwr", "media.html"),
        ("flamewm-audio.rwr", "audio.html"),
        ("flamewm-network.rwr", "network.html"),
        ("flamewm-calendar.rwr", "calendar.html"),
        ("flamewm-osd.rwr", "osd.html"),
        ("flamewm-launch-feedback.rwr", "launch-feedback.html"),
        ("flamewm-battery.rwr", "battery.html"),
        ("flamewm-weather.rwr", "weather.html"),
        ("flamewm-notification.rwr", "notification.html"),
        (
            "flamewm-notification-center.rwr",
            "notification-center.html",
        ),
        ("flamewm-launcher.rwr", "launcher.html"),
        ("flamewm-switcher.rwr", "switcher.html"),
        ("flamewm-polkit.rwr", "polkit.html"),
    ];

    println!("cargo:rerun-if-changed={}", css.display());
    println!("cargo:rerun-if-changed={}", menu_css.display());
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
    // Shell pages and flamewm.css reference ../base/tokens.css relative to OUT_DIR/shell.
    let tokens = root.join("ui/base/tokens.css");
    println!("cargo:rerun-if-changed={}", tokens.display());
    fs::create_dir_all(out_dir.join("base")).expect("create staged base dir");
    fs::copy(&tokens, out_dir.join("base/tokens.css")).expect("stage token CSS");
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
        compile_to(&staged_ui.join(source), &menu_css, &out_dir.join(artifact));
    }
}

fn compile_to(source: &Path, shared_menu_css: &Path, output: &Path) {
    let compiled = compile_file(
        source,
        &[shared_menu_css.to_path_buf()],
        CompileOptions::default(),
    )
    .unwrap_or_else(|error| panic!("failed to compile {}: {error}", source.display()));
    let bytes = encode(&compiled.document).expect("compiled FlameWM UI must encode");
    fs::write(output, bytes).expect("write compiled FlameWM UI artifact");
}
