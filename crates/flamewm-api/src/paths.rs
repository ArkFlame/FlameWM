//! XDG base-directory resolution from the process environment.

use std::env;
use std::path::{Path, PathBuf};

/// `$HOME` as a path, when set (empty values included, matching the historical callers).
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

/// `$var` when set and non-empty, else `$HOME/<home_relative>`. `None` when neither resolves.
#[must_use]
pub fn xdg_dir(var: &str, home_relative: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(home_relative)))
}

/// `usr/share/flamewm` for an executable installed as `usr/bin/<name>`.
#[must_use]
pub fn installed_data_root(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    let usr_dir = bin_dir.parent()?;
    (bin_dir.file_name()? == "bin" && usr_dir.file_name()? == "usr")
        .then(|| usr_dir.join("share/flamewm"))
}

/// Packaged data root: the installed tree when the running binary has one with `assets/`,
/// otherwise the checkout `manifest_root`.
#[must_use]
pub fn packaged_root(manifest_root: &Path) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|exe| installed_data_root(&exe))
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or_else(|| manifest_root.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_binary_maps_to_sibling_share_data_root() {
        assert_eq!(
            installed_data_root(Path::new("/opt/flamewm/usr/bin/flamewm-desktop")),
            Some(PathBuf::from("/opt/flamewm/usr/share/flamewm"))
        );
    }

    #[test]
    fn development_executable_has_no_installed_root() {
        assert_eq!(
            installed_data_root(Path::new("/workspace/flamewm/target/debug/flamewm-desktop")),
            None
        );
    }
}
