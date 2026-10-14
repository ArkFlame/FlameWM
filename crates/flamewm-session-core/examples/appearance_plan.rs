//! Materialize the session appearance plan into an isolated config home and
//! print the XDG_CONFIG_DIRS and QT_QPA_PLATFORMTHEME values to export (empty
//! when not set). Used by the toolkit probe.
//!
//! Usage: appearance_plan <config_home> <dark|light|system> [inherited XDG_CONFIG_DIRS]

use std::collections::BTreeMap;
use std::path::PathBuf;

use flamewm_api::settings::AppearanceMode;
use flamewm_session_core::{
    AppearanceProbe, SessionPaths, kde_platform_theme_installed, materialize_owned_files,
    plan_appearance,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(config_home), Some(mode)) = (args.first(), args.get(1)) else {
        eprintln!("usage: appearance_plan <config_home> <dark|light|system> [xdg_config_dirs]");
        std::process::exit(2);
    };
    let mode = AppearanceMode::parse(mode).unwrap_or_else(|| {
        eprintln!("unknown mode");
        std::process::exit(2);
    });
    let paths = SessionPaths {
        bindir: PathBuf::from("/usr/bin"),
        config_home: PathBuf::from(config_home),
        data_root: PathBuf::from("/usr/share"),
    };
    let inherited: BTreeMap<String, String> = args
        .get(2)
        .map(|value| BTreeMap::from([("XDG_CONFIG_DIRS".to_owned(), value.clone())]))
        .unwrap_or_default();
    let probe = AppearanceProbe {
        kde_platform_theme: kde_platform_theme_installed(&inherited),
        ..AppearanceProbe::default()
    };
    let plan = plan_appearance(&paths, &inherited, mode, probe);
    if let Err(error) = materialize_owned_files(&plan) {
        eprintln!("materialize failed: {error}");
        std::process::exit(1);
    }
    for key in ["XDG_CONFIG_DIRS", "QT_QPA_PLATFORMTHEME"] {
        println!("{key}={}", plan.environment.get(key).map_or("", String::as_str));
    }
}
