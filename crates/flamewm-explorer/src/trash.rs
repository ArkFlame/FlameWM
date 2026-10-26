//! Freedesktop.org Trash view model: list `info/*.trashinfo`, restore and empty.
//! Emptying reuses `flamewm_desktop_core::trash::Trash::empty`; this module owns listing and restore.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_TRASH_ENTRIES: usize = 10_000;
const MAX_INFO_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashEntry {
    /// Stored file inside `files/`.
    pub stored: PathBuf,
    /// The `.trashinfo` file inside `info/`.
    pub info: PathBuf,
    /// Original absolute location recorded in the info file.
    pub original: PathBuf,
    /// Raw `DeletionDate=` value (local time, `YYYY-MM-DDThh:mm:ss`).
    pub deleted: String,
}

impl TrashEntry {
    #[must_use]
    pub fn name(&self) -> String {
        self.stored
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

/// Percent-decodes a `Path=` value. Returns `None` for malformed escapes or invalid UTF-8.
#[must_use]
pub fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            let digit = |v: u8| match v {
                b'0'..=b'9' => Some(v - b'0'),
                b'a'..=b'f' => Some(v - b'a' + 10),
                b'A'..=b'F' => Some(v - b'A' + 10),
                _ => None,
            };
            out.push(digit(hi)? * 16 + digit(lo)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parses the contents of a `.trashinfo` file into `(original path, deletion date)`.
#[must_use]
pub fn parse_trashinfo(text: &str) -> Option<(PathBuf, String)> {
    let mut in_section = false;
    let mut path = None;
    let mut date = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_section {
                break;
            }
            in_section = line == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(value) = line.strip_prefix("Path=") {
            path = percent_decode(value.trim());
        } else if let Some(value) = line.strip_prefix("DeletionDate=") {
            date = value
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(40)
                .collect();
        }
    }
    let path = path.filter(|p| !p.is_empty() && p.contains('/') && !p.contains('\0'))?;
    Some((PathBuf::from(path), date))
}

/// Lists trash entries for a trash base directory (the one containing `files/` and `info/`).
/// Entries whose info file is missing or unparseable are skipped; the result is capped.
pub fn list(base: &Path) -> io::Result<Vec<TrashEntry>> {
    let info_dir = base.join("info");
    let files_dir = base.join("files");
    let mut entries = Vec::new();
    for item in fs::read_dir(&info_dir)? {
        let item = item?;
        let info = item.path();
        if info.extension().and_then(|e| e.to_str()) != Some("trashinfo") {
            continue;
        }
        let Some(stem) = info.file_stem().map(PathBuf::from) else {
            continue;
        };
        let stored = files_dir.join(&stem);
        if stored.symlink_metadata().is_err() {
            continue;
        }
        let Ok(meta) = item.metadata() else { continue };
        if meta.len() > MAX_INFO_BYTES {
            continue;
        }
        let Ok(text) = fs::read_to_string(&info) else {
            continue;
        };
        let Some((original, deleted)) = parse_trashinfo(&text) else {
            continue;
        };
        entries.push(TrashEntry {
            stored,
            info,
            original,
            deleted,
        });
        if entries.len() >= MAX_TRASH_ENTRIES {
            break;
        }
    }
    entries.sort_by(|a, b| {
        b.deleted
            .cmp(&a.deleted)
            .then_with(|| a.name().cmp(&b.name()))
    });
    Ok(entries)
}

/// Returns a destination path that does not exist, appending ` (restored N)` before the extension.
#[must_use]
pub fn unique_destination(original: &Path) -> PathBuf {
    if !original.exists() && original.symlink_metadata().is_err() {
        return original.to_path_buf();
    }
    let parent = original.parent().unwrap_or(Path::new("/")).to_path_buf();
    let stem = original
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ext = original
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1u32.. {
        let candidate = parent.join(format!("{stem} (restored {n}){ext}"));
        if candidate.symlink_metadata().is_err() {
            return candidate;
        }
    }
    unreachable!("u32 range exhausted")
}

/// Restores one entry to its original location (never overwriting) and removes its info file.
/// Returns the final destination. Cross-device restores fail with an error and leave the trash intact.
pub fn restore(entry: &TrashEntry) -> io::Result<PathBuf> {
    if !entry.original.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Trash record has no absolute original path",
        ));
    }
    if let Some(parent) = entry.original.parent() {
        fs::create_dir_all(parent)?;
    }
    let destination = unique_destination(&entry.original);
    fs::rename(&entry.stored, &destination)?;
    if let Err(error) = fs::remove_file(&entry.info) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("flamewm-trash-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("files")).unwrap();
        fs::create_dir_all(base.join("info")).unwrap();
        base
    }

    #[test]
    fn parses_percent_encoded_paths_and_rejects_malformed() {
        assert_eq!(percent_decode("/a%20b/c%C3%A9").as_deref(), Some("/a b/cé"));
        assert_eq!(percent_decode("/bad%2"), None);
        let (path, date) = parse_trashinfo(
            "[Trash Info]\nPath=/home/u/a%20b.txt\nDeletionDate=2026-10-21T10:00:00\n",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/u/a b.txt"));
        assert_eq!(date, "2026-10-21T10:00:00");
        assert!(parse_trashinfo("[Trash Info]\nPath=relative\n").is_none());
        assert!(parse_trashinfo("Path=/x\n").is_none());
    }

    #[test]
    fn lists_restores_without_overwriting_and_skips_orphans() {
        let base = temp_base("restore");
        let origin = base.join("origin");
        fs::create_dir_all(&origin).unwrap();
        let original = origin.join("note.txt");
        fs::write(base.join("files/note.txt"), b"trashed").unwrap();
        fs::write(
            base.join("info/note.txt.trashinfo"),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-10-21T09:00:00\n",
                original.display()
            ),
        )
        .unwrap();
        fs::write(
            base.join("info/orphan.trashinfo"),
            "[Trash Info]\nPath=/x/y\n",
        )
        .unwrap();
        fs::write(&original, b"occupied").unwrap();

        let entries = list(&base).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "orphan info without stored file must be skipped"
        );
        let restored = restore(&entries[0]).unwrap();
        assert_eq!(restored, origin.join("note (restored 1).txt"));
        assert_eq!(fs::read(&restored).unwrap(), b"trashed");
        assert_eq!(fs::read(&original).unwrap(), b"occupied");
        assert!(!base.join("info/note.txt.trashinfo").exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn move_to_trash_then_restore_round_trips_through_xdg_layout() {
        let root =
            std::env::temp_dir().join(format!("flamewm-trash-roundtrip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let data = root.join("data");
        fs::create_dir_all(home.join("Docs")).unwrap();
        let file = home.join("Docs/report.txt");
        fs::write(&file, b"keep me").unwrap();
        let uid = fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| flamewm_desktop_core::trash::current_uid_from_proc(&s))
            .expect("uid");

        let item =
            flamewm_desktop_core::trash::Trash::move_to_trash(&file, &home, &data, uid).unwrap();
        assert!(!file.exists(), "source must leave its folder");
        assert!(item.stored.starts_with(data.join("Trash/files")));
        assert!(item.info.starts_with(data.join("Trash/info")));

        let base = data.join("Trash");
        let entries = list(&base).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original, file);
        assert_eq!(fs::read(&entries[0].stored).unwrap(), b"keep me");

        let restored = restore(&entries[0]).unwrap();
        assert_eq!(restored, file);
        assert_eq!(fs::read(&file).unwrap(), b"keep me");
        assert!(list(&base).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_original_is_refused() {
        let entry = TrashEntry {
            stored: PathBuf::from("/nonexistent/files/a"),
            info: PathBuf::from("/nonexistent/info/a.trashinfo"),
            original: PathBuf::from("relative/a"),
            deleted: String::new(),
        };
        assert_eq!(
            restore(&entry).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
