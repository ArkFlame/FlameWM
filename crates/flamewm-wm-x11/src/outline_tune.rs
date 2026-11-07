//! INTERNAL: dev-only live outline tuning; remove or keep behind the env gate.
//!
//! `FLAMEWM_OUTLINE_TUNE=<path>` names a `key=value` text file (`width=2`, `radius=12`,
//! `alpha=128`) that overrides the settings-derived window outline for every frame. The file is
//! re-read only when its mtime or size changed, so a paint never pays more than one `stat`.
//!
//! Unset or empty: nothing is read and no syscall is made; the caller keeps the settings values
//! byte for byte. This is an internal tool, not a product feature: no settings key, no UI.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::UNIX_EPOCH;

/// Environment variable naming the tune file.
const ENV: &str = "FLAMEWM_OUTLINE_TUNE";

/// One resolved override set. `None` keeps the settings-derived value for that field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OutlineTune {
    pub(crate) width: Option<u32>,
    pub(crate) radius: Option<u32>,
    pub(crate) alpha: Option<u8>,
}

/// Parse the tune file. Malformed lines and unknown keys are ignored; out-of-range values clamp
/// to the documented bounds (`width` 1..=4, `radius` 0..=48, `alpha` 0..=255).
#[must_use]
pub(crate) fn parse(text: &str) -> OutlineTune {
    let mut tune = OutlineTune::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<u32>() else {
            continue;
        };
        match key.trim() {
            "width" => tune.width = Some(value.clamp(1, 4)),
            "radius" => tune.radius = Some(value.min(48)),
            "alpha" => tune.alpha = Some(value.min(255) as u8),
            _ => {}
        }
    }
    tune
}

/// Cached file identity and the tune it produced, plus the last value reported to the event loop.
struct State {
    path: String,
    /// `(mtime nanos, length)` of the last read; `None` until read or after a failed stat.
    stamp: Option<(u128, u64)>,
    tune: OutlineTune,
    published: OutlineTune,
}

impl State {
    fn new() -> Self {
        Self {
            path: String::new(),
            stamp: None,
            tune: OutlineTune::default(),
            published: OutlineTune::default(),
        }
    }

    /// Re-read the tune file only when its identity changed.
    fn sync(&mut self, path: &str) {
        if self.path != path {
            path.clone_into(&mut self.path);
            self.stamp = None;
            // The overrides belong to the previous file; a new path starts from the defaults.
            self.tune = OutlineTune::default();
        }
        let stamp = file_stamp(Path::new(path));
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        self.tune = std::fs::read_to_string(path)
            .map_or_else(|_| OutlineTune::default(), |text| parse(&text));
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tune file named by the environment; `None` when unset or empty (no file is touched).
fn env_path() -> Option<String> {
    match std::env::var(ENV) {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    }
}

fn file_stamp(path: &Path) -> Option<(u128, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, metadata.len()))
}

/// Current overrides. One `stat` at most per call, and none when the environment is unset.
#[must_use]
pub(crate) fn current() -> OutlineTune {
    let Some(path) = env_path() else {
        return OutlineTune::default();
    };
    let mut guard = state();
    let state = guard.get_or_insert_with(State::new);
    state.sync(&path);
    state.tune
}

/// The overrides when they changed since the previous poll, so the event loop re-shapes and
/// repaints once per edit instead of every tick. Clearing the variable also reports a change, so
/// the settings values own the outline again.
#[must_use]
pub(crate) fn poll() -> Option<OutlineTune> {
    let path = env_path();
    let mut guard = state();
    // Unset and never tuned: no state, no file, nothing to report.
    if path.is_none()
        && guard
            .as_ref()
            .is_none_or(|state| state.published == OutlineTune::default())
    {
        return None;
    }
    let state = guard.get_or_insert_with(State::new);
    match path {
        Some(path) => state.sync(&path),
        None => state.tune = OutlineTune::default(),
    }
    if state.published == state.tune {
        return None;
    }
    state.published = state.tune;
    Some(state.tune)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_keys_are_read() {
        let tune = parse("width=3\nradius=12\nalpha=128\n");
        assert_eq!(
            tune,
            OutlineTune {
                width: Some(3),
                radius: Some(12),
                alpha: Some(128),
            }
        );
    }

    #[test]
    fn out_of_range_values_clamp() {
        let tune = parse("width=99\nradius=400\nalpha=999");
        assert_eq!(
            tune,
            OutlineTune {
                width: Some(4),
                radius: Some(48),
                alpha: Some(255),
            }
        );
        assert_eq!(parse("width=0\nradius=0\nalpha=0").width, Some(1));
        assert_eq!(parse("radius=0").radius, Some(0));
        assert_eq!(parse("alpha=0").alpha, Some(0));
    }

    #[test]
    fn malformed_lines_and_unknown_keys_are_ignored() {
        let tune = parse("width\n=2\nwidth=abc\nbogus=7\n\nradius = 9 \nalpha=+1");
        assert_eq!(
            tune,
            OutlineTune {
                width: None,
                radius: Some(9),
                alpha: Some(1),
            }
        );
        assert_eq!(parse("").width, None);
    }

    /// The stat cache: an unchanged file keeps the parsed value, a changed one is re-read.
    #[test]
    fn the_file_is_re_read_only_when_its_identity_changes() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-outline-tune-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, "width=2\n").expect("write tune file");
        let mut state = State::new();
        state.sync(path.to_str().expect("utf-8 temp path"));
        assert_eq!(state.tune.width, Some(2));
        // Same identity: the cached tune survives even though the file content differs.
        std::fs::write(&path, "width=3\n").expect("rewrite tune file");
        let stamp = file_stamp(&path);
        state.stamp = stamp;
        state.sync(path.to_str().expect("utf-8 temp path"));
        assert_eq!(state.tune.width, Some(2), "no stat change, no re-read");
        // A new identity is read again.
        state.stamp = None;
        state.sync(path.to_str().expect("utf-8 temp path"));
        assert_eq!(state.tune.width, Some(3));
        // A missing file clears the overrides.
        state.stamp = None;
        state.path.clear();
        state.sync("/nonexistent/flamewm-outline-tune.txt");
        assert_eq!(state.tune, OutlineTune::default());
        let _ = std::fs::remove_file(path);
    }
}
