use std::{env,fs,path::{Path,PathBuf}};
use flamewm_render_compiler::{CompileOptions,compile_file,encode};
fn main() {
    let base=PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let root=base.parent().and_then(Path::parent).expect("workspace root");
    let src=root.join("ui/explorer");
    let html=src.join("explorer.html");
    let css=src.join("explorer.css");
    println!("cargo:rerun-if-changed={}",html.display());
    println!("cargo:rerun-if-changed={}",css.display());
    let dest=PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let staged=dest.join("explorer");
    fs::create_dir_all(&staged).expect("stage UI");
    fs::copy(&html,staged.join("explorer.html")).expect("stage HTML");
    fs::copy(&css,staged.join("explorer.css")).expect("stage CSS");
    let compiled=compile_file(&staged.join("explorer.html"),&[],CompileOptions::default())
        .unwrap_or_else(|error|panic!("FlameWM Explorer UI compile: {error}"));
    fs::write(Path::new(&dest).join("flamewm-explorer.rwr"), encode(&compiled.document).expect("encode UI")).expect("write compiled UI");
}
