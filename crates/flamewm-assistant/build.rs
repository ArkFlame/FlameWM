use flamewm_render_compiler::{CompileOptions, compile_file, encode};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let src = base.join("../../ui/assistant");
    for file in ["assistant.html", "assistant.css", "osd.html", "osd.css", "icons"] {
        println!("cargo:rerun-if-changed={}", src.join(file).display());
    }
    println!("cargo:rerun-if-changed={}", src.join("../base/base.css").display());
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    for (html, name) in [("assistant.html", "flamewm-assistant.rwr"), ("osd.html", "flamewm-assistant-osd.rwr")] {
        let result = compile_file(&src.join(html), &[], CompileOptions::default())
            .unwrap_or_else(|error| panic!("FlameWM Assistant UI {html}: {error}"));
        fs::write(Path::new(&out).join(name), encode(&result.document).expect("encode UI"))
            .expect("write UI");
    }
}
