use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{CompileOptions, compile_file, encode};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let version_path = root.join("FLAMEWM_VERSION");
    let version_text = fs::read_to_string(&version_path).expect("read FlameWM version");
    let version = version_text.trim();
    assert!(!version.is_empty(), "FlameWM version must not be empty");
    println!("cargo:rustc-env=FLAMEWM_SETTINGS_VERSION={version}");
    println!("cargo:rerun-if-changed={}", version_path.display());

    let ui = root.join("ui/settings.html");
    let css = root.join("ui/settings.css");
    let web = root.join("assets/web");
    let breeze = root.join("assets/web/breeze");
    let branding = root.join("assets/branding");

    println!("cargo:rerun-if-changed={}", ui.display());
    println!("cargo:rerun-if-changed={}", css.display());
    println!("cargo:rerun-if-changed={}", breeze.display());
    println!("cargo:rerun-if-changed={}", branding.display());

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged_ui = out_dir.join("settings");
    let staged_assets = staged_ui.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged settings assets");
    fs::copy(&ui, staged_ui.join("settings.html")).expect("stage settings UI");
    fs::copy(&css, staged_ui.join("settings.css")).expect("stage settings CSS");
    stage_tree(&breeze, &staged_assets.join("breeze"));
    stage_tree(&branding, &staged_assets.join("branding"));
    let staged_web = staged_assets.join("web");
    fs::create_dir_all(&staged_web).expect("create staged settings web assets");
    for name in ["flamewm-icon.svg", "github.svg", "paypal.svg"] {
        fs::copy(web.join(name), staged_assets.join("web").join(name))
            .unwrap_or_else(|_| panic!("stage settings web asset {name}"));
    }

    let staged_html = staged_ui.join("settings.html");
    let compiled = compile_file(&staged_html, &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("failed to compile {}: {error}", ui.display()));
    let output = out_dir.join("flamewm-settings.rwr");
    fs::write(
        output,
        encode(&compiled.document).expect("encode settings UI"),
    )
    .expect("write settings UI");
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
