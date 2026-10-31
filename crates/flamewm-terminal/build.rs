use std::{
    env, fs,
    path::{Path, PathBuf},
};

use flamewm_render_compiler::{CompileOptions, compile_file, encode};

include!("src/pool.rs");

fn main() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = base
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let src = root.join("ui/terminal");
    let template = src.join("terminal.html");
    let css = src.join("terminal.css");
    println!("cargo:rerun-if-changed={}", template.display());
    println!("cargo:rerun-if-changed={}", css.display());
    println!("cargo:rerun-if-changed={}", root.join("ui/base/base.css").display());
    println!(
        "cargo:rerun-if-changed={}",
        base.join("src/pool.rs").display()
    );
    let dest = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let staged = dest.join("terminal");
    fs::create_dir_all(&staged).expect("stage UI");
    fs::copy(&css, staged.join("terminal.css")).expect("stage CSS");
    fs::create_dir_all(dest.join("base")).expect("stage base dir");
    fs::copy(root.join("ui/base/base.css"), dest.join("base/base.css")).expect("stage base CSS");
    let template = fs::read_to_string(&template).expect("read terminal.html");
    for (tier, pool) in TIERS.iter().enumerate() {
        let mut runs = String::new();
        for row in 0..pool.rows {
            for slot in 0..pool.runs {
                // One text child per run node: the runtime rewrites it with set_text, hidden slots keep this placeholder (whitespace-only text is dropped by the compiler).
                runs.push_str(&format!(
                    "<span id=\"r{row}s{slot}\" class=\"run\">-</span>"
                ));
            }
        }
        let html = template.replace("<!--RUNS-->", &runs);
        let page = staged.join(format!("terminal-{tier}.html"));
        fs::write(&page, html).expect("stage HTML");
        let compiled = compile_file(&page, &[], CompileOptions::default())
            .unwrap_or_else(|error| panic!("FlameWM Terminal UI compile: {error}"));
        fs::write(
            dest.join(format!("flamewm-terminal-{tier}.rwr")),
            encode(&compiled.document).expect("encode UI"),
        )
        .expect("write compiled UI");
    }
}
