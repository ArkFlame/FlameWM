//! Secure theme importer. Every source is parsed into an in-memory plan and fully
//! validated before the first write. Writes go to a staging dir inside the icons
//! dir, which is then renamed into place. Nothing is ever executed.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::ThemeKind;
use super::archive::{self, RawEntry, RawKind};
use super::read_regular_capped;
use crate::icon_theme::IconThemeIndex;

pub(crate) const MAX_ENTRIES: usize = 10_000;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ID_LEN: usize = 64;
const MAX_LINK_HOPS: usize = 40;
/// Crashed-import leftovers older than this are swept at the next import.
const STALE_AGE: Duration = Duration::from_secs(60 * 60);
const STAGING_PREFIX: &[u8] = b".flamewm-staging-";
const REPLACED_PREFIX: &[u8] = b".flamewm-replaced-";

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    UnsupportedFormat(String),
    UnsafePath(String),
    UnsupportedEntry(String),
    EscapingLink(String),
    InvalidUtf8,
    MissingIndex,
    AmbiguousThemes(Vec<String>),
    IdCollision(String),
    InvalidId(String),
    InvalidTheme(String),
    Corrupt(String),
    TooManyEntries,
    TooDeep,
    TooLarge,
    Cancelled,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ImportError {}

impl From<io::Error> for ImportError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Clone, Debug, Default)]
pub struct InstallOptions {
    /// Install name. Also selects one theme when the source holds several.
    pub id: Option<String>,
    pub overwrite: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledTheme {
    pub id: String,
    pub kind: ThemeKind,
    pub root: std::path::PathBuf,
}

#[derive(Debug)]
enum PlanKind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
    /// Archive-relative source path while building; theme-relative once planned.
    HardLink(String),
}

struct Plan {
    id: String,
    kind: ThemeKind,
    entries: Vec<(String, PlanKind)>,
}

/// Install a `.tar.gz` archive or an extracted folder into `icons_dir`.
pub fn install_theme(
    source: &Path,
    icons_dir: &Path,
    options: &InstallOptions,
    cancel: &AtomicBool,
) -> Result<InstalledTheme, ImportError> {
    let raw = load_source(source, cancel)?;
    // An extracted folder is itself the theme, so its name is the fallback id.
    let folder_name = source
        .is_dir()
        .then(|| source.file_name().and_then(|n| n.to_str()).map(str::to_owned))
        .flatten();
    let plan = build_plan(raw, options.id.as_deref(), folder_name.as_deref(), cancel)?;
    let dest = icons_dir.join(&plan.id);
    if !options.overwrite && fs::symlink_metadata(&dest).is_ok() {
        return Err(ImportError::IdCollision(plan.id));
    }
    check_cancel(cancel)?;
    fs::create_dir_all(icons_dir)?;
    // Best effort: a failed sweep must not block the import.
    let _ = sweep_stale(icons_dir, STALE_AGE);
    let staging = icons_dir.join(format!(".flamewm-staging-{}-{}", std::process::id(), stamp()));
    fs::create_dir(&staging)?;
    let outcome = write_plan(&staging, &plan.entries, cancel)
        .and_then(|()| commit(&staging, &dest, icons_dir, &plan.id, options.overwrite, cancel));
    if outcome.is_err() {
        let _ = remove_entry(&staging);
    }
    outcome?;
    Ok(InstalledTheme {
        id: plan.id,
        kind: plan.kind,
        root: dest,
    })
}

/// Stop a long import step once the caller has cancelled.
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), ImportError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ImportError::Cancelled);
    }
    Ok(())
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// Remove `.flamewm-staging-*` and `.flamewm-replaced-*` directories left by a
/// crashed import. Only real directories directly inside `icons_dir` whose mtime
/// is at least `max_age` old are removed; symlinks are never followed.
fn sweep_stale(icons_dir: &Path, max_age: Duration) -> io::Result<()> {
    let now = SystemTime::now();
    for item in fs::read_dir(icons_dir)? {
        let item = item?;
        let name = item.file_name();
        let bytes = name.as_bytes();
        if !(bytes.starts_with(STAGING_PREFIX) || bytes.starts_with(REPLACED_PREFIX)) {
            continue;
        }
        let path = item.path();
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age >= max_age {
            fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(_) => Ok(()),
    }
}

fn commit(
    staging: &Path,
    dest: &Path,
    icons_dir: &Path,
    id: &str,
    overwrite: bool,
    cancel: &AtomicBool,
) -> Result<(), ImportError> {
    check_cancel(cancel)?;
    if fs::symlink_metadata(dest).is_err() {
        fs::rename(staging, dest)?;
        return Ok(());
    }
    if !overwrite {
        return Err(ImportError::IdCollision(id.to_owned()));
    }
    let backup = icons_dir.join(format!(".flamewm-replaced-{}-{}", std::process::id(), stamp()));
    fs::rename(dest, &backup)?;
    if let Err(err) = fs::rename(staging, dest) {
        let _ = fs::rename(&backup, dest);
        return Err(err.into());
    }
    let _ = remove_entry(&backup);
    Ok(())
}

fn write_plan(
    staging: &Path,
    entries: &[(String, PlanKind)],
    cancel: &AtomicBool,
) -> Result<(), ImportError> {
    // Real files and dirs first, so hard links can point at staged files.
    let real = entries
        .iter()
        .filter(|(_, k)| matches!(k, PlanKind::Dir | PlanKind::File(_)));
    let links = entries
        .iter()
        .filter(|(_, k)| matches!(k, PlanKind::Symlink(_) | PlanKind::HardLink(_)));
    for (rel, kind) in real.chain(links) {
        check_cancel(cancel)?;
        let path = staging.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match kind {
            PlanKind::Dir => fs::create_dir_all(&path)?,
            PlanKind::File(data) => {
                let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
                file.write_all(data)?;
            }
            PlanKind::Symlink(target) => symlink(target, &path)?,
            PlanKind::HardLink(src) => fs::hard_link(staging.join(src), &path)?,
        }
    }
    Ok(())
}

fn load_source(source: &Path, cancel: &AtomicBool) -> Result<Vec<RawEntry>, ImportError> {
    if fs::metadata(source)?.is_dir() {
        return walk_folder(source, cancel);
    }
    let name = source.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let file = File::open(source)?;
        let archive_bytes = archive::read_decoded(flate2::read::GzDecoder::new(file), cancel, "gzip")?;
        return archive::parse(&archive_bytes, MAX_ENTRIES, cancel);
    }
    if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        return archive::parse_xz(File::open(source)?, cancel);
    }
    Err(ImportError::UnsupportedFormat(name.to_owned()))
}

fn walk_folder(root: &Path, cancel: &AtomicBool) -> Result<Vec<RawEntry>, ImportError> {
    let mut out: Vec<RawEntry> = Vec::new();
    let mut total: u64 = 0;
    let mut stack: Vec<(std::path::PathBuf, Vec<u8>, usize)> = vec![(root.to_path_buf(), Vec::new(), 0)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        for item in fs::read_dir(&dir)? {
            check_cancel(cancel)?;
            let item = item?;
            if depth + 1 > MAX_DEPTH {
                return Err(ImportError::TooDeep);
            }
            if out.len() >= MAX_ENTRIES {
                return Err(ImportError::TooManyEntries);
            }
            let mut rel = prefix.clone();
            if !rel.is_empty() {
                rel.push(b'/');
            }
            rel.extend_from_slice(item.file_name().as_bytes());
            let path = item.path();
            let file_type = item.file_type()?;
            if file_type.is_dir() {
                out.push(RawEntry {
                    path: rel.clone(),
                    kind: RawKind::Dir,
                    data: Vec::new(),
                });
                stack.push((path, rel, depth + 1));
            } else if file_type.is_file() {
                // Cap on bytes actually read, so a file that grows or is swapped
                // after the directory listing cannot exceed the budget.
                let Some(data) = read_regular_capped(&path, MAX_TOTAL_BYTES - total)? else {
                    return Err(ImportError::TooLarge);
                };
                total += data.len() as u64;
                out.push(RawEntry {
                    path: rel,
                    kind: RawKind::File,
                    data,
                });
            } else if file_type.is_symlink() {
                let target = fs::read_link(&path)?;
                out.push(RawEntry {
                    path: rel,
                    kind: RawKind::Symlink(target.as_os_str().as_bytes().to_vec()),
                    data: Vec::new(),
                });
            } else {
                return Err(ImportError::UnsupportedEntry(
                    String::from_utf8_lossy(&rel).into_owned(),
                ));
            }
        }
    }
    Ok(out)
}

fn utf8(bytes: &[u8]) -> Result<&str, ImportError> {
    std::str::from_utf8(bytes).map_err(|_| ImportError::InvalidUtf8)
}

/// Lexically clean an archive path. `Ok(None)` means the archive root itself.
fn sanitize_path(bytes: &[u8]) -> Result<Option<String>, ImportError> {
    let text = utf8(bytes)?;
    if text.starts_with('/') {
        return Err(ImportError::UnsafePath(text.to_owned()));
    }
    let mut parts = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(ImportError::UnsafePath(text.to_owned())),
            p if p.contains('\0') => return Err(ImportError::UnsafePath(text.to_owned())),
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    if parts.len() > MAX_DEPTH {
        return Err(ImportError::TooDeep);
    }
    Ok(Some(parts.join("/")))
}

fn within_root(key: &str, root: &str) -> bool {
    root.is_empty()
        || key == root
        || key
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

fn join_key(root: &str, rel: &str) -> String {
    if root.is_empty() {
        rel.to_owned()
    } else {
        format!("{root}/{rel}")
    }
}

fn validate_id(id: &str) -> Result<(), ImportError> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'));
    if ok {
        Ok(())
    } else {
        Err(ImportError::InvalidId(id.to_owned()))
    }
}

/// Follow a symlink target lexically, resolving intermediate links inside the
/// plan. Every step must stay inside `root`. `None` means it escapes or loops.
fn resolve_link(files: &BTreeMap<String, PlanKind>, root: &str, link: &str, target: &str) -> Option<()> {
    if target.starts_with('/') {
        return None;
    }
    let mut cur: Vec<String> = link.split('/').map(str::to_owned).collect();
    cur.pop();
    let mut pending: VecDeque<String> = target.split('/').map(str::to_owned).collect();
    let mut hops = 0usize;
    while let Some(part) = pending.pop_front() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                cur.pop()?;
            }
            _ => {
                cur.push(part);
                if let Some(PlanKind::Symlink(next)) = files.get(&cur.join("/")) {
                    hops += 1;
                    if hops > MAX_LINK_HOPS || next.starts_with('/') {
                        return None;
                    }
                    cur.pop();
                    for comp in next.split('/').rev() {
                        pending.push_front(comp.to_owned());
                    }
                }
            }
        }
        if !within_root(&cur.join("/"), root) {
            return None;
        }
    }
    if cur.is_empty() { None } else { Some(()) }
}

fn build_plan(
    raw: Vec<RawEntry>,
    requested_id: Option<&str>,
    folder_name: Option<&str>,
    cancel: &AtomicBool,
) -> Result<Plan, ImportError> {
    let mut files: BTreeMap<String, PlanKind> = BTreeMap::new();
    for entry in raw {
        check_cancel(cancel)?;
        let Some(rel) = sanitize_path(&entry.path)? else {
            continue;
        };
        let kind = match entry.kind {
            RawKind::Dir => PlanKind::Dir,
            RawKind::File => PlanKind::File(entry.data),
            RawKind::Symlink(target) => PlanKind::Symlink(utf8(&target)?.to_owned()),
            RawKind::Hardlink(target) => PlanKind::HardLink(
                sanitize_path(&target)?.ok_or_else(|| ImportError::UnsafePath(rel.clone()))?,
            ),
            RawKind::Other(code) => {
                return Err(ImportError::UnsupportedEntry(format!("{rel} (type {code})")));
            }
        };
        if files.insert(rel.clone(), kind).is_some() {
            return Err(ImportError::Corrupt(format!("duplicate path {rel}")));
        }
        if files.len() > MAX_ENTRIES {
            return Err(ImportError::TooManyEntries);
        }
    }

    // Nothing may live below a non-directory entry.
    for key in files.keys() {
        for (idx, _) in key.match_indices('/') {
            if files
                .get(&key[..idx])
                .is_some_and(|kind| !matches!(kind, PlanKind::Dir))
            {
                return Err(ImportError::Corrupt(format!("{key} is below a non-directory")));
            }
        }
    }

    let candidates: Vec<String> = files
        .iter()
        .filter_map(|(key, kind)| {
            if !matches!(kind, PlanKind::File(_)) {
                return None;
            }
            if key == "index.theme" {
                Some(String::new())
            } else {
                key.strip_suffix("/index.theme").map(str::to_owned)
            }
        })
        .collect();
    let root = if candidates.is_empty() {
        return Err(ImportError::MissingIndex);
    } else if let [only] = candidates.as_slice() {
        only.clone()
    } else if let Some(id) = requested_id {
        candidates
            .iter()
            .find(|candidate| basename(candidate) == id)
            .cloned()
            .ok_or_else(|| ImportError::InvalidTheme(format!("no theme directory named {id}")))?
    } else {
        return Err(ImportError::AmbiguousThemes(candidates));
    };

    let id = match requested_id {
        Some(id) => id.to_owned(),
        None => {
            let name = match basename(&root) {
                "" => folder_name.unwrap_or(""),
                name => name,
            };
            if name.is_empty() {
                return Err(ImportError::InvalidTheme(
                    "theme at archive root needs an explicit id".into(),
                ));
            }
            name.to_owned()
        }
    };
    validate_id(&id)?;

    let index_key = join_key(&root, "index.theme");
    let Some(PlanKind::File(index_bytes)) = files.get(&index_key) else {
        return Err(ImportError::MissingIndex);
    };
    let text = std::str::from_utf8(index_bytes)
        .map_err(|_| ImportError::InvalidTheme("index.theme is not UTF-8".into()))?;
    let has_icon = !IconThemeIndex::parse(&id, text).directories.is_empty();
    let cursors_prefix = join_key(&root, "cursors/");
    let has_cursor = files.keys().any(|k| k.starts_with(&cursors_prefix));
    let kind = if has_icon {
        ThemeKind::Icon
    } else if has_cursor {
        ThemeKind::Cursor
    } else {
        return Err(ImportError::InvalidTheme(
            "neither Directories= nor cursors/ present".into(),
        ));
    };

    for (key, kind) in &files {
        if key == &root || !within_root(key, &root) {
            continue;
        }
        match kind {
            PlanKind::Symlink(target) => {
                if resolve_link(&files, &root, key, target).is_none() {
                    return Err(ImportError::EscapingLink(key.clone()));
                }
            }
            PlanKind::HardLink(src) => {
                if !within_root(src, &root) || !matches!(files.get(src), Some(PlanKind::File(_))) {
                    return Err(ImportError::EscapingLink(key.clone()));
                }
            }
            PlanKind::Dir | PlanKind::File(_) => {}
        }
    }

    let mut entries = Vec::new();
    for (key, kind) in files {
        if key == root || !within_root(&key, &root) {
            continue;
        }
        let rel = if root.is_empty() {
            key
        } else {
            key[root.len() + 1..].to_owned()
        };
        let kind = match kind {
            PlanKind::HardLink(src) => {
                PlanKind::HardLink(if root.is_empty() { src } else { src[root.len() + 1..].to_owned() })
            }
            other => other,
        };
        entries.push((rel, kind));
    }
    Ok(Plan { id, kind, entries })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use super::{REPLACED_PREFIX, STAGING_PREFIX, STALE_AGE, sweep_stale};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-sweep-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn age_dir(path: &Path, age: Duration) {
        let when = SystemTime::now() - age;
        fs::File::open(path).and_then(|f| f.set_modified(when)).expect("set mtime");
    }

    #[test]
    fn stale_staging_and_replaced_dirs_are_swept_but_fresh_and_foreign_entries_stay() {
        let icons = scratch("sweep");
        let stale_staging = icons.join(format!("{}1-2", std::str::from_utf8(STAGING_PREFIX).unwrap()));
        let stale_replaced = icons.join(format!("{}1-2", std::str::from_utf8(REPLACED_PREFIX).unwrap()));
        let fresh_staging = icons.join(format!("{}9-9", std::str::from_utf8(STAGING_PREFIX).unwrap()));
        let theme = icons.join("Adwaita");
        let foreign = icons.join(".flamewm-other-1");
        for dir in [&stale_staging, &stale_replaced, &fresh_staging, &theme, &foreign] {
            fs::create_dir_all(dir.join("keep")).expect("dir");
        }
        age_dir(&stale_staging, STALE_AGE * 2);
        age_dir(&stale_replaced, STALE_AGE * 2);
        age_dir(&theme, STALE_AGE * 2);
        age_dir(&foreign, STALE_AGE * 2);

        sweep_stale(&icons, STALE_AGE).expect("sweep");

        assert!(!stale_staging.exists(), "stale staging dir must be swept");
        assert!(!stale_replaced.exists(), "stale replaced dir must be swept");
        assert!(fresh_staging.exists(), "fresh staging dir may belong to a live import");
        assert!(theme.exists(), "installed themes are never swept");
        assert!(foreign.exists(), "only the exact prefixes are swept");
        let _ = fs::remove_dir_all(&icons);
    }

    #[test]
    fn symlink_named_like_staging_is_never_followed_or_removed_recursively() {
        let icons = scratch("sweep-link");
        let outside = scratch("sweep-link-target");
        fs::write(outside.join("precious"), b"keep").expect("target file");
        let link = icons.join(format!("{}3-4", std::str::from_utf8(STAGING_PREFIX).unwrap()));
        symlink(&outside, &link).expect("symlink");
        age_dir(&outside, STALE_AGE * 2);

        sweep_stale(&icons, STALE_AGE).expect("sweep");

        assert!(outside.join("precious").exists(), "sweep must not follow the symlink");
        let _ = fs::remove_dir_all(&icons);
        let _ = fs::remove_dir_all(&outside);
    }
}
