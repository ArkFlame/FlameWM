use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{CompileOptions, CompileOutput, compile_file, decode, encode};

#[path = "ui_contract.rs"]
mod ui_contract;

const DESKTOP_ASSETS: [&str; 2] = ["wallpaper.ppm", "watermark.png"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("ui_contract.rs").display()
    );
    let html_path = manifest.join("../../ui/desktop/index.html");
    let css_path = manifest.join("../../ui/desktop/desktop.css");
    let menu_css_path = manifest.join("../../ui/shared/menu.css");
    let raster = root.join("assets/raster");
    let wallpaper = root.join("assets/wallpapers/fitz-roy-dawn.webp");
    let breeze = root.join("assets/web/breeze");
    println!("cargo:rerun-if-changed={}", html_path.display());
    println!("cargo:rerun-if-changed={}", css_path.display());
    println!("cargo:rerun-if-changed={}", menu_css_path.display());
    rerun_tree(&breeze);
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
<div id=\"desktop-label-{index}\" class=\"desktop-label\"><div id=\"desktop-label-{index}-line-1\" class=\"desktop-label-line\">&#8203;</div><div id=\"desktop-label-{index}-line-2\" class=\"desktop-label-line\">&#8203;</div></div>\
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
    // These three local menu glyphs are not part of the selectable system icon
    // themes. Always bundle them with the desktop RWR so the same UI is shown
    // with PlagueSur, Fluent and a minimal installed asset directory.
    let menu_icons = root.join("ui/desktop/icons");
    for icon in [
        "flamewm-convert.svg",
        "flamewm-add-workspace.svg",
        "flamewm-new-folder.svg",
    ] {
        let source = menu_icons.join(icon);
        println!("cargo:rerun-if-changed={}", source.display());
        fs::copy(source, staged_assets.join("breeze").join(icon))
            .expect("stage FlameWM-owned menu icon");
    }
    let staged_wallpapers = staged_assets.join("wallpapers");
    fs::create_dir_all(&staged_wallpapers).expect("create staged wallpaper dir");
    fs::copy(&wallpaper, staged_wallpapers.join("fitz-roy-dawn.webp"))
        .expect("stage default wallpaper");

    let output = compile_file(
        &staged_ui.join("index.html"),
        &[css_path, menu_css_path],
        CompileOptions::default(),
    )
    .expect("compile desktop UI");
    validate_text_targets(&output);
    let compiled_ids = output
        .document
        .nodes
        .iter()
        .map(|node| &*node.id)
        .collect::<Vec<_>>();
    validate_compiled_desktop_nodes(&compiled_ids);
    let bytes = encode(&output.document).expect("encode desktop UI");
    // This is the exact serialization boundary that the runtime reads via
    // include_bytes!(OUT_DIR/flamewm-desktop.rwr). HTML text is not proof.
    let decoded = decode(&bytes).expect("decode encoded desktop RWR");
    let decoded_ids = decoded
        .nodes
        .iter()
        .map(|node| &*node.id)
        .collect::<Vec<_>>();
    validate_compiled_desktop_nodes(&decoded_ids);
    fs::write(out.join("flamewm-desktop.rwr"), bytes).expect("write validated desktop UI");
}

fn validate_compiled_desktop_nodes(ids: &[&str]) {
    ui_contract::require_desktop_nodes(|id| ids.contains(&id)).unwrap_or_else(|error| {
        panic!("{error}; verify ui/desktop/index.html and rebuild flamewm-desktop")
    });
    for required in ui_contract::REQUIRED_DESKTOP_IDS {
        assert_eq!(
            ids.iter().filter(|id| *id == required).count(),
            1,
            "desktop compiled RWR must contain exactly one node with id '{required}'"
        );
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

fn validate_text_targets(output: &CompileOutput) {
    let mut ids = Vec::with_capacity(272);
    for slot in 1..=16 {
        ids.push(format!("sticky-{slot}-text"));
    }
    for index in 0..128 {
        ids.push(format!("desktop-label-{index}-line-1"));
        ids.push(format!("desktop-label-{index}-line-2"));
    }
    for slot in 0..8 {
        let id = format!("convert-slot-{slot}");
        assert_dynamic_text_target(output, &id);
    }
    assert_dynamic_text_target(output, "convert-wheel-hub-text");
    assert_dynamic_text_target(output, "confirm-text");
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

/// Dynamic button labels/hub text must be a single text child; otherwise
/// RuntimeDocument::set_text fails at the first right-click during startup.
fn assert_dynamic_text_target(output: &CompileOutput, id: &str) {
    let node = output
        .document
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("desktop UI is missing dynamic text target '{id}'"));
    let first = node
        .first_child
        .expect("dynamic text target requires one text child");
    let child = &output.document.nodes[first as usize];
    assert!(
        child.next_sibling.is_none() && child.first_child.is_none(),
        "dynamic text target '{id}' must contain one text child"
    );
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
