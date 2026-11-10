use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{CompileOptions, compile_file, encode, rerun_tree, stage_tree};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    assert!(!version.is_empty(), "FlameWM version must not be empty");
    println!("cargo:rustc-env=FLAMEWM_SETTINGS_VERSION={version}");

    let ui = root.join("ui/settings.html");
    let css = root.join("ui/settings.css");
    let web = root.join("assets/web");
    let breeze = root.join("assets/web/breeze");
    let branding = root.join("assets/branding");

    println!("cargo:rerun-if-changed={}", ui.display());
    println!("cargo:rerun-if-changed={}", css.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("ui/settings-accessibility.css").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("ui/base/base.css").display()
    );
    rerun_tree(&breeze);
    rerun_tree(&branding);
    for name in [
        "flamewm-icon.svg",
        "github.svg",
        "paypal.svg",
        "color-wheel.png",
    ] {
        println!("cargo:rerun-if-changed={}", web.join(name).display());
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged_ui = out_dir.join("settings");
    let staged_assets = staged_ui.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged settings assets");
    fs::copy(&ui, staged_ui.join("settings.html")).expect("stage settings UI");
    fs::copy(&css, staged_ui.join("settings.css")).expect("stage settings CSS");
    fs::copy(
        root.join("ui/settings-accessibility.css"),
        staged_ui.join("settings-accessibility.css"),
    )
    .expect("stage settings accessibility CSS");
    fs::create_dir_all(staged_ui.join("base")).expect("stage base dir");
    fs::copy(
        root.join("ui/base/base.css"),
        staged_ui.join("base/base.css"),
    )
    .expect("stage base CSS");
    // base.css imports tokens.css, which must sit beside it in the staged tree.
    fs::copy(
        root.join("ui/base/tokens.css"),
        staged_ui.join("base/tokens.css"),
    )
    .expect("stage token CSS");
    stage_tree(&breeze, &staged_assets.join("breeze"));
    stage_tree(&branding, &staged_assets.join("branding"));
    let staged_web = staged_assets.join("web");
    fs::create_dir_all(&staged_web).expect("create staged settings web assets");
    for name in [
        "flamewm-icon.svg",
        "github.svg",
        "paypal.svg",
        "color-wheel.png",
    ] {
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
