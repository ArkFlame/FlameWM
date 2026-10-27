use flamewm_render_compiler::{CompileOptions, compile_file, encode};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let src = base.join("../../ui/task-manager");
    for file in ["task-manager.html", "task-manager.css"] {
        println!("cargo:rerun-if-changed={}", src.join(file).display());
    }
    let result = compile_file(
        &src.join("task-manager.html"),
        &[],
        CompileOptions::default(),
    )
    .unwrap_or_else(|error| panic!("FlameWM Task Manager UI: {error}"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(
        Path::new(&out).join("flamewm-task-manager.rwr"),
        encode(&result.document).expect("encode UI"),
    )
    .expect("write UI");
}
