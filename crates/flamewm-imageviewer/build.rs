use flamewm_render_compiler::{CompileOptions, compile_file, encode};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let src = base.join("../../ui/imageviewer");
    for file in ["imageviewer.html", "imageviewer.css", "blank.ppm"] {
        println!("cargo:rerun-if-changed={}", src.join(file).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        base.join("../../ui/base/base.css").display()
    );
    let result = compile_file(
        &src.join("imageviewer.html"),
        &[],
        CompileOptions::default(),
    )
    .unwrap_or_else(|error| panic!("FlameWM Image Viewer UI: {error}"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(
        Path::new(&out).join("flamewm-imageviewer.rwr"),
        encode(&result.document).expect("encode UI"),
    )
    .expect("write UI");
}
