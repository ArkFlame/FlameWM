//! XDG base-directory resolution from the process environment.

use std::env;
use std::path::PathBuf;

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
