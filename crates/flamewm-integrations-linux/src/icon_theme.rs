//! Freedesktop `index.theme` parser and inheritance expansion (J2).

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
mod tests;

mod dirs;
pub use dirs::*;
mod index;
pub use index::*;
mod caches;
pub use caches::*;
mod walk;
pub use walk::*;
mod lookup;
pub use lookup::*;
mod watch;
use watch::*;
mod theme_roots;
pub use theme_roots::*;
mod file_index;
pub use file_index::*;
