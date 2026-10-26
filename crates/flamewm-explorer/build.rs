use flamewm_render_compiler::{CompileOptions, compile_file, encode};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = base
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let src = root.join("ui/explorer");
    let html = src.join("explorer.html");
    let css = src.join("explorer.css");
    println!("cargo:rerun-if-changed={}", html.display());
    println!("cargo:rerun-if-changed={}", css.display());
    let icons = src.join("icons");
    println!("cargo:rerun-if-changed={}", icons.display());
    let dest = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let staged = dest.join("explorer");
    fs::create_dir_all(&staged).expect("stage UI");
    fs::copy(&html, staged.join("explorer.html")).expect("stage HTML");
    fs::copy(&css, staged.join("explorer.css")).expect("stage CSS");
    // Images resolve relative to the staged HTML, so the bundled SVG icons ship beside it.
    let staged_icons = staged.join("icons");
    fs::create_dir_all(&staged_icons).expect("stage icons");
    for entry in fs::read_dir(&icons).expect("read icons") {
        let entry = entry.expect("icon entry");
        println!("cargo:rerun-if-changed={}", entry.path().display());
        fs::copy(entry.path(), staged_icons.join(entry.file_name())).expect("stage icon");
    }
    let compiled = compile_file(
        &staged.join("explorer.html"),
        &[],
        CompileOptions::default(),
    )
    .unwrap_or_else(|error| panic!("FlameWM Explorer UI compile: {error}"));
    fs::write(
        Path::new(&dest).join("flamewm-explorer.rwr"),
        encode(&compiled.document).expect("encode UI"),
    )
    .expect("write compiled UI");
}
