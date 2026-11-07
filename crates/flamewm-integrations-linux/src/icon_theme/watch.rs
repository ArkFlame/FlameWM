//! Watch-directory selection with depth and count caps.

use super::*;

/// Depth below each search root that is watched for new icons.
pub(super) const WATCH_DEPTH: usize = 3;
/// Upper bound on directories stat-ed per watch check (across all depths).
pub(super) const WATCH_DIR_CAP: usize = 2048;
/// Per-depth share of `WATCH_DIR_CAP`, so a wide shallow level cannot starve
/// the deeper category levels of every theme.
pub(super) const WATCH_LEVEL_CAP: usize = WATCH_DIR_CAP / (WATCH_DEPTH + 1);
/// Set once per process so a capped watch set logs a single diagnostic line.
pub(super) static WATCH_CAP_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Directories whose mtime changes when an icon is added or removed:
/// every search root plus theme subdirectories (`theme/size/category`) up to
/// `WATCH_DEPTH`. Walks level by level; each level keeps at most
/// `WATCH_LEVEL_CAP` dirs, interleaved round-robin across parents so every
/// theme's shallow levels are covered before any one theme consumes a level.
/// Data dirs are stat-only and not counted against the cap.
pub(super) fn watch_dirs(
    roots: &[PathBuf],
    data_dirs: &[PathBuf],
    listings: Option<&DirListings>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = data_dirs.to_vec();
    let mut level: Vec<PathBuf> = roots.to_vec();
    let mut capped = false;
    for depth in 0..=WATCH_DEPTH {
        if level.len() > WATCH_LEVEL_CAP {
            level.truncate(WATCH_LEVEL_CAP);
            capped = true;
        }
        if depth == WATCH_DEPTH {
            out.append(&mut level);
            break;
        }
        let groups: Vec<Vec<PathBuf>> = level.iter().map(|dir| child_dirs(dir, listings)).collect();
        out.append(&mut level);
        level = interleave_by_parent(&groups);
    }
    if capped {
        log_watch_cap_once();
    }
    out
}

pub(super) fn child_dirs(dir: &Path, listings: Option<&DirListings>) -> Vec<PathBuf> {
    dir_items(dir, listings)
        .iter()
        .filter(|item| item.is_dir)
        .map(|item| item.path.clone())
        .collect()
}

/// Round-robin merge of per-parent child lists: first child of every parent,
/// then second child of every parent, and so on.
pub(super) fn interleave_by_parent(groups: &[Vec<PathBuf>]) -> Vec<PathBuf> {
    let rounds = groups.iter().map(Vec::len).max().unwrap_or(0);
    (0..rounds)
        .flat_map(|index| {
            groups
                .iter()
                .filter_map(move |group| group.get(index).cloned())
        })
        .collect()
}

pub(super) fn log_watch_cap_once() {
    if !WATCH_CAP_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "flamewm-integrations-linux: icon watch set capped at {WATCH_LEVEL_CAP} dirs per depth (total cap {WATCH_DIR_CAP}); deeper icon dir changes may not trigger a refresh"
        );
    }
}

pub(super) fn current_watch_mtime(
    roots: &[PathBuf],
    data_dirs: &[PathBuf],
    listings: Option<&DirListings>,
) -> Option<std::time::SystemTime> {
    watch_dirs(roots, data_dirs, listings)
        .iter()
        .filter_map(|top| fs::metadata(top).and_then(|m| m.modified()).ok())
        .max()
}
