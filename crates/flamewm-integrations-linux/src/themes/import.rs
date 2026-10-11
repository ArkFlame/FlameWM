//! Secure theme importer. Every source is parsed into an in-memory plan and fully
//! validated before the first write. Writes go to a staging dir inside the icons
//! dir, which is then renamed into place. Nothing is ever executed.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ThemeKind;
use super::archive::{self, RawEntry, RawKind};
use crate::icon_theme::IconThemeIndex;

pub(crate) const MAX_ENTRIES: usize = 10_000;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ID_LEN: usize = 64;
const MAX_LINK_HOPS: usize = 40;

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
    if cancel.load(Ordering::Relaxed) {
        return Err(ImportError::Cancelled);
    }
    fs::create_dir_all(icons_dir)?;
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

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
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
    if cancel.load(Ordering::Relaxed) {
        return Err(ImportError::Cancelled);
    }
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
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
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
        let mut decoder = flate2::read::GzDecoder::new(file).take(MAX_TOTAL_BYTES + 1);
        let mut archive_bytes = Vec::new();
        decoder
            .read_to_end(&mut archive_bytes)
            .map_err(|err| ImportError::Corrupt(format!("gzip stream: {err}")))?;
        if archive_bytes.len() as u64 > MAX_TOTAL_BYTES {
            return Err(ImportError::TooLarge);
        }
        return archive::parse(&archive_bytes, MAX_ENTRIES, cancel);
    }
    if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        return Err(ImportError::UnsupportedFormat(
            "xz: no offline pure-Rust decoder is available".into(),
        ));
    }
    Err(ImportError::UnsupportedFormat(name.to_owned()))
}

fn walk_folder(root: &Path, cancel: &AtomicBool) -> Result<Vec<RawEntry>, ImportError> {
    let mut out: Vec<RawEntry> = Vec::new();
    let mut total: u64 = 0;
    let mut stack: Vec<(std::path::PathBuf, Vec<u8>, usize)> = vec![(root.to_path_buf(), Vec::new(), 0)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        for item in fs::read_dir(&dir)? {
            if cancel.load(Ordering::Relaxed) {
                return Err(ImportError::Cancelled);
            }
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
                total = total.saturating_add(item.metadata()?.len());
                if total > MAX_TOTAL_BYTES {
                    return Err(ImportError::TooLarge);
                }
                out.push(RawEntry {
                    path: rel,
                    kind: RawKind::File,
                    data: fs::read(&path)?,
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
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
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
