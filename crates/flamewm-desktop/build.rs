use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{CompileOptions, CompileOutput, compile_file, encode};

const DESKTOP_ASSETS: [&str; 2] = ["wallpaper.ppm", "watermark.png"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let html_path = manifest.join("../../ui/desktop/index.html");
    let css_path = manifest.join("../../ui/desktop/desktop.css");
    let raster = root.join("assets/raster");
    let wallpaper = root.join("assets/branding/flamewm-wallpaper.webp");
    let breeze = root.join("assets/web/breeze");
    println!("cargo:rerun-if-changed={}", html_path.display());
    println!("cargo:rerun-if-changed={}", css_path.display());
    println!("cargo:rerun-if-changed={}", breeze.display());
    println!("cargo:rerun-if-changed={}", wallpaper.display());
    assert!(
        wallpaper.is_file(),
        "required packaged default wallpaper is missing: {}",
        wallpaper.display()
    );
    for asset in DESKTOP_ASSETS {
        println!("cargo:rerun-if-changed={}", raster.join(asset).display());
    }

    let html = fs::read_to_string(&html_path).expect("read desktop UI");
    let items = (0..128)
        .map(|index| {
            format!(
                "<button id=\"desktop-item-{index}\" class=\"desktop-item\" data-action=\"desktop.item.{index}\">\
<img id=\"desktop-glyph-{index}-desktop-launcher\" class=\"desktop-glyph\" src=\"assets/breeze/internet-web-browser.svg\">\
<img id=\"desktop-glyph-{index}-file\" class=\"desktop-glyph\" src=\"assets/breeze/folder-documents.svg\">\
<img id=\"desktop-glyph-{index}-symlink\" class=\"desktop-glyph\" src=\"assets/breeze/folder.svg\">\
<img id=\"desktop-glyph-{index}-trash\" class=\"desktop-glyph\" src=\"assets/breeze/user-trash.svg\">\
<img id=\"desktop-glyph-{index}-directory\" class=\"desktop-glyph\" src=\"assets/breeze/folder.svg\">\
<div class=\"desktop-label\"><div id=\"desktop-label-{index}-line-1\" class=\"desktop-label-line\">&#8203;</div><div id=\"desktop-label-{index}-line-2\" class=\"desktop-label-line\">&#8203;</div></div>\
</button>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    let html = html.replace("<!--ITEMS-->", &items);

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged_ui = out.join("desktop");
    let staged_assets = staged_ui.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged desktop assets");
    fs::write(staged_ui.join("index.html"), html).expect("write staged desktop UI");
    for asset in DESKTOP_ASSETS {
        fs::copy(raster.join(asset), staged_assets.join(asset)).expect("stage desktop asset");
    }
    stage_tree(&breeze, &staged_assets.join("breeze"));

    let output = compile_file(
        &staged_ui.join("index.html"),
        &[css_path],
        CompileOptions::default(),
    )
    .expect("compile desktop UI");
    validate_text_targets(&output);
    let bytes = encode(&output.document).expect("encode desktop UI");
    fs::write(out.join("flamewm-desktop.rwr"), bytes).expect("write desktop UI");
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

fn validate_text_targets(output: &CompileOutput) {
    let mut ids = Vec::with_capacity(272);
    for slot in 1..=16 {
        ids.push(format!("sticky-{slot}-text"));
    }
    for index in 0..128 {
        ids.push(format!("desktop-label-{index}-line-1"));
        ids.push(format!("desktop-label-{index}-line-2"));
    }
    for id in &ids {
        let position = output
            .document
            .nodes
            .iter()
            .position(|node| node.id == *id)
            .unwrap_or_else(|| panic!("desktop UI is missing runtime text target '{id}'"));
        let first = output.document.nodes[position]
            .first_child
            .unwrap_or_else(|| panic!("desktop UI text target '{id}' has no text child"));
        let child = &output.document.nodes[first as usize];
        if child.first_child.is_some()
            || child.next_sibling.is_some()
            || !child.text.contains('\u{200b}')
        {
            panic!("desktop UI text target '{id}' is not a single-text-node element");
        }
    }
}
