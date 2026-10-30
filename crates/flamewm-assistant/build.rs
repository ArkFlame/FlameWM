use flamewm_render_compiler::{CompileOptions, compile_file, encode};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let src = base.join("../../ui/assistant");
    for file in ["assistant.html", "assistant.css", "icons"] {
        println!("cargo:rerun-if-changed={}", src.join(file).display());
    }
    let result = compile_file(&src.join("assistant.html"), &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("FlameWM Assistant UI: {error}"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(
        Path::new(&out).join("flamewm-assistant.rwr"),
        encode(&result.document).expect("encode UI"),
    )
    .expect("write UI");
}
