//! Resolves the packaged data root: the installed `usr/share/flamewm` tree or the workspace checkout.

use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn packaged_root() -> PathBuf {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("flamewm workspace root");
    resolve_packaged_root(env::current_exe().ok().as_deref(), &manifest_root, |root| {
        root.join("assets").is_dir()
    })
}

pub(crate) fn resolve_packaged_root(
    executable: Option<&Path>,
    manifest_root: &Path,
    assets_available: impl FnOnce(&Path) -> bool,
) -> PathBuf {
    executable
        .and_then(installed_data_root)
        .filter(|root| assets_available(root))
        .unwrap_or_else(|| manifest_root.to_path_buf())
}

pub(crate) fn installed_data_root(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    let usr_dir = bin_dir.parent()?;
    (bin_dir.file_name()? == "bin" && usr_dir.file_name()? == "usr")
        .then(|| usr_dir.join("share/flamewm"))
}
