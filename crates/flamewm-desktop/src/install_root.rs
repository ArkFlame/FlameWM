//! Packaged data root for the desktop process, resolved by the shared `flamewm-api` owner.

use std::path::{Path, PathBuf};

pub(crate) fn packaged_root() -> PathBuf {
    flamewm_api::paths::packaged_root(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").as_path())
}
