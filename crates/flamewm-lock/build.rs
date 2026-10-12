use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{compile_file, encode, CompileOptions};

const LOCK_ICONS: [&str; 5] = [
    "system-suspend.svg",
    "system-reboot.svg",
    "system-shutdown.svg",
    "system-switch-user.svg",
    "system-log-out.svg",
];

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
    let web = root.join("assets/web");
    let lock_icon_src = web.join("breeze/lock");
    for icon in LOCK_ICONS {
        println!(
            "cargo:rerun-if-changed={}",
            lock_icon_src.join(icon).display()
        );
    }
    let avatar_src = web.join("lock/avatar-outline.svg");
    println!("cargo:rerun-if-changed={}", avatar_src.display());

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

    // Lock-screen icons (near-white #EAF0FF variants) and the outline avatar,
    // staged under assets/breeze/ and assets/lock/ for the lock document.
    let staged_breeze = staged_assets.join("breeze");
    fs::create_dir_all(&staged_breeze).expect("create staged lock breeze icons");
    for icon in LOCK_ICONS {
        fs::copy(lock_icon_src.join(icon), staged_breeze.join(icon))
            .unwrap_or_else(|error| panic!("failed to stage lock icon {icon}: {error}"));
    }
    let staged_lock = staged_assets.join("lock");
    fs::create_dir_all(&staged_lock).expect("create staged lock avatar dir");
    fs::copy(&avatar_src, staged_lock.join("avatar-outline.svg"))
        .expect("stage lock avatar outline");
    println!(
        "cargo:rustc-env=FLAMEWM_LOCK_STAGED_ASSETS={}",
        staged_assets.display()
    );

    let compiled = compile_file(&staged.join("lock.html"), &[], CompileOptions::default())
        .unwrap_or_else(|error| panic!("failed to compile lock.html: {error}"));
    let bytes = encode(&compiled.document).expect("compiled FlameWM lock UI must encode");
    fs::write(out_dir.join("flamewm-lock.rwr"), bytes).expect("write compiled lock UI artifact");
}
