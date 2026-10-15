use std::env;
use std::fs;
use std::path::PathBuf;

use flamewm_render_compiler::{CompileOptions, compile_file, encode};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let html = manifest.join("ui/snap-preview.html");
    let css = manifest.join("ui/snap-preview.css");
    println!("cargo:rerun-if-changed={}", html.display());
    println!("cargo:rerun-if-changed={}", css.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("ui/snap-preview-blank.ppm").display()
    );
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let compiled = compile_file(&html, &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("failed to compile {}: {error}", html.display()));
    fs::write(
        out_dir.join("flamewm-snap-preview.rwr"),
        encode(&compiled.document).expect("encode snap preview UI"),
    )
    .expect("write snap preview UI");
}
