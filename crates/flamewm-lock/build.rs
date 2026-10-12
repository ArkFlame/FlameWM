use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{compile_file, encode, CompileOptions};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let ui = root.join("ui/lock");
    println!("cargo:rerun-if-changed={}", ui.join("lock.html").display());
    println!("cargo:rerun-if-changed={}", ui.join("lock.css").display());
    println!(
        "cargo:rerun-if-changed={}",
        ui.join("assets/blank.svg").display()
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged = out_dir.join("lock");
    let staged_assets = staged.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged lock UI assets");
    for source in ["lock.html", "lock.css"] {
        fs::copy(ui.join(source), staged.join(source)).expect("stage lock UI source");
    }
    fs::copy(
        ui.join("assets/blank.svg"),
        staged_assets.join("blank.svg"),
    )
    .expect("stage lock UI blank image");
    let compiled = compile_file(&staged.join("lock.html"), &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("failed to compile lock.html: {error}"));
    let bytes = encode(&compiled.document).expect("compiled FlameWM lock UI must encode");
    fs::write(out_dir.join("flamewm-lock.rwr"), bytes).expect("write compiled lock UI artifact");
}
