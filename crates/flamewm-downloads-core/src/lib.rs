//! Downloads organizer policy: extension classification, collision-free names, the once-only
//! notice latch and the no-replace move into category folders. The desktop process owns the
//! inotify watch and scheduling; nothing here touches X11 or the reactor.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// Exact text of the one-time notice shown after the first real move.
pub const NOTICE_TEXT: &str = "FlameWM organizes your downloads automatically into category folders. You can disable this in the settings.";

/// Persisted boolean setting that enables the organizer.
pub const SETTING_KEY: &str = "organizeDownloads";

/// A file that stopped changing for this long is moved even without a close/rename signal.
pub const STABLE_AFTER: std::time::Duration = std::time::Duration::from_millis(1500);

/// Poll cadence while a candidate file is still settling.
pub const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Upper bound of moves per scheduling tick so a burst cannot starve the UI loop.
pub const MAX_MOVES_PER_TICK: usize = 32;

const MAX_NAME_ATTEMPTS: u32 = 100;
const PARTIAL_SUFFIXES: &[&str] = &[
    "crdownload",
    "part",
    "download",
    "tmp",
    "opdownload",
    "partial",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Documents,
    Text,
    Multimedia,
    Archives,
    Packages,
}

impl Category {
    #[must_use]
    pub const fn folder_name(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Text => "Text",
            Self::Multimedia => "Multimedia",
            Self::Archives => "Archives",
            Self::Packages => "Packages",
        }
    }
}

const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "odt", "ott", "rtf", "xls", "xlsx", "xlsm", "ods", "ppt", "pptx", "odp",
    "csv", "tsv", "epub", "mobi", "azw3", "djvu",
];
const TEXT: &[&str] = &[
    "txt", "md", "markdown", "bbcode", "js", "mjs", "cjs", "ts", "tsx", "jsx", "json", "xml",
    "yaml", "yml", "toml", "ini", "cfg", "conf", "log", "html", "htm", "css", "scss", "rs", "py",
    "c", "cc", "cpp", "cxx", "h", "hpp", "java", "kt", "go", "rb", "php", "sh", "bash", "zsh",
    "fish", "lua", "sql", "tex",
];
const MULTIMEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff", "avif", "heic", "heif",
    "ico", "raw", "cr2", "nef", "arw", "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv",
    "mpg", "mpeg", "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma",
];
const ARCHIVES: &[&str] = &[
    "zip", "tar", "gz", "tgz", "xz", "txz", "bz2", "tbz2", "7z", "rar", "zst", "lz", "lzma", "lz4",
    "cab",
];
const PACKAGES: &[&str] = &[
    "deb",
    "rpm",
    "appimage",
    "flatpakref",
    "flatpak",
    "snap",
    "iso",
    "exe",
    "msi",
    "dmg",
    "pkg",
];

/// Lowercase final extension of a plain file name, or `None` without a usable stem.
fn extension(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| extension.to_ascii_lowercase())
}

/// True for names the organizer may ever act on: not hidden, not a browser/editor temp or lock
/// name, and not a path-like string.
#[must_use]
pub fn is_candidate_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('.')
        || name.starts_with('~')
        || name.ends_with('~')
        || name.contains(['/', '\0'])
    {
        return false;
    }
    !extension(name).is_some_and(|ext| PARTIAL_SUFFIXES.contains(&ext.as_str()))
}

/// Category for a finished download name, or `None` when it stays in Downloads.
#[must_use]
pub fn classify(name: &str) -> Option<Category> {
    if !is_candidate_name(name) {
        return None;
    }
    let extension = extension(name)?;
    let extension = extension.as_str();
    [
        (DOCUMENTS, Category::Documents),
        (TEXT, Category::Text),
        (MULTIMEDIA, Category::Multimedia),
        (ARCHIVES, Category::Archives),
        (PACKAGES, Category::Packages),
    ]
    .into_iter()
    .find(|(list, _)| list.contains(&extension))
    .map(|(_, category)| category)
}

/// First name that `exists` reports free, keeping the extension: `a.pdf`, `a (1).pdf`, ...
#[must_use]
pub fn unique_name(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    for index in 1..u32::MAX {
        let candidate = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{name} ({index})"),
        };
        if !exists(&candidate) {
            return candidate;
        }
    }
    name.to_owned()
}

/// Resolves the Downloads directory from `user-dirs.dirs` text, falling back to `~/Downloads`.
/// The file is parsed as data, never executed as shell.
#[must_use]
pub fn downloads_dir(home: &Path, user_dirs: Option<&str>) -> PathBuf {
    let Some(text) = user_dirs else {
        return home.join("Downloads");
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some(value) = line.strip_prefix("XDG_DOWNLOAD_DIR=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if let Some(suffix) = value.strip_prefix("$HOME") {
            return home.join(suffix.trim_start_matches('/'));
        }
        if let Some(suffix) = value.strip_prefix("${HOME}") {
            return home.join(suffix.trim_start_matches('/'));
        }
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return path;
        }
    }
    home.join("Downloads")
}

/// Once-only latch for the first-move notice. `claim` returns true exactly once per profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoticeLatch {
    shown: bool,
}

impl NoticeLatch {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self {
            shown: text.lines().any(|line| line.trim() == "shown=1"),
        }
    }

    #[must_use]
    pub fn serialize(self) -> String {
        format!("shown={}\n", u8::from(self.shown))
    }

    #[must_use]
    pub const fn shown(self) -> bool {
        self.shown
    }

    /// Marks the notice as taken and reports whether this call is the one that may show it.
    pub fn claim(&mut self) -> bool {
        !std::mem::replace(&mut self.shown, true)
    }
}

/// Reads the latch; a missing or unreadable file means the notice was never shown.
#[must_use]
pub fn load_notice(path: &Path) -> NoticeLatch {
    fs::read_to_string(path)
        .map(|text| NoticeLatch::parse(&text))
        .unwrap_or_default()
}

/// Atomically persists the latch (temp file, fsync, rename) so a crash cannot repeat the notice.
pub fn persist_notice(path: &Path, latch: NoticeLatch) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = File::create(&temp)?;
    io::Write::write_all(&mut file, latch.serialize().as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

enum RenameFailure {
    Exists,
    CrossDevice,
    Failed(io::Error),
}

fn rename_no_replace(from: &Path, to: &Path) -> Result<(), RenameFailure> {
    use rustix::io::Errno;
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(RenameFailure::Exists),
        Err(Errno::XDEV) => Err(RenameFailure::CrossDevice),
        // Filesystems without RENAME_NOREPLACE: check-then-rename, bounded by the caller's retry.
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {
            if fs::symlink_metadata(to).is_ok() {
                Err(RenameFailure::Exists)
            } else {
                fs::rename(from, to).map_err(RenameFailure::Failed)
            }
        }
        Err(other) => Err(RenameFailure::Failed(io::Error::from_raw_os_error(
            other.raw_os_error(),
        ))),
    }
}

/// Cross-device fallback: copy to a hidden partial beside the target, fsync, publish with
/// no-replace, then remove the source. The source survives any failure before its removal.
fn copy_then_commit(
    source: &Path,
    folder: &Path,
    target_name: &str,
    target: &Path,
) -> Result<(), RenameFailure> {
    let partial = folder.join(format!(".{target_name}.flamewm-partial"));
    let copied = fs::copy(source, &partial).and_then(|_| File::open(&partial)?.sync_all());
    if let Err(error) = copied {
        let _ = fs::remove_file(&partial);
        return Err(RenameFailure::Failed(error));
    }
    match rename_no_replace(&partial, target) {
        Ok(()) => fs::remove_file(source).map_err(RenameFailure::Failed),
        Err(failure) => {
            let _ = fs::remove_file(&partial);
            Err(failure)
        }
    }
}

/// Moves `dir/name` into `dir/<category>/`, creating the folder when missing and reusing it when
/// present. Never replaces an existing entry (`name (1).ext` on clashes) and returns the final path.
pub fn move_into_category(dir: &Path, name: &str, category: Category) -> io::Result<PathBuf> {
    let source = dir.join(name);
    let folder = dir.join(category.folder_name());
    match fs::symlink_metadata(&folder) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "category path exists and is not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&folder)?,
        Err(error) => return Err(error),
    }
    for _ in 0..MAX_NAME_ATTEMPTS {
        let target_name = unique_name(name, |candidate| {
            fs::symlink_metadata(folder.join(candidate)).is_ok()
        });
        let target = folder.join(&target_name);
        let outcome = match rename_no_replace(&source, &target) {
            Err(RenameFailure::CrossDevice) => {
                copy_then_commit(&source, &folder, &target_name, &target)
            }
            other => other,
        };
        match outcome {
            Ok(()) => return Ok(target),
            Err(RenameFailure::Exists) => continue,
            Err(RenameFailure::CrossDevice) => {
                return Err(io::Error::other("cross-device move did not complete"));
            }
            Err(RenameFailure::Failed(error)) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name in category folder",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_root(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn classifies_extensions_case_insensitively_into_categories() {
        assert_eq!(classify("report.PDF"), Some(Category::Documents));
        assert_eq!(classify("notes.md"), Some(Category::Text));
        assert_eq!(classify("Photo.JPG"), Some(Category::Multimedia));
        assert_eq!(classify("clip.MkV"), Some(Category::Multimedia));
        assert_eq!(classify("backup.tar.gz"), Some(Category::Archives));
        assert_eq!(classify("app.AppImage"), Some(Category::Packages));
        assert_eq!(classify("disk.iso"), Some(Category::Packages));
    }

    #[test]
    fn uncategorized_and_extensionless_names_stay_put() {
        assert_eq!(classify("Makefile"), None);
        assert_eq!(classify("data.xyz"), None);
        assert_eq!(classify("trailing."), None);
    }

    #[test]
    fn temp_partial_and_hidden_names_are_never_candidates() {
        for name in [
            "a.pdf.crdownload",
            "b.zip.part",
            "c.mp4.download",
            "d.txt.tmp",
            "e.pdf.opdownload",
            "f.iso.partial",
            ".hidden.pdf",
            ".com.google.Chrome.abc123",
            ".~lock.report.odt#",
            "~scratch.txt",
            "backup.txt~",
            "",
        ] {
            assert!(!is_candidate_name(name), "{name} must be ignored");
            assert_eq!(classify(name), None, "{name} must not classify");
        }
        assert!(is_candidate_name("finished.pdf"));
    }

    #[test]
    fn unique_name_keeps_extension_and_skips_taken_suffixes() {
        assert_eq!(unique_name("a.pdf", |_| false), "a.pdf");
        let taken = ["a.pdf", "a (1).pdf"];
        assert_eq!(unique_name("a.pdf", |n| taken.contains(&n)), "a (2).pdf");
        assert_eq!(unique_name("LICENSE", |n| n == "LICENSE"), "LICENSE (1)");
        assert_eq!(unique_name("x.tar.gz", |n| n == "x.tar.gz"), "x.tar (1).gz");
    }

    #[test]
    fn downloads_dir_follows_user_dirs_and_falls_back_to_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            downloads_dir(home, None),
            PathBuf::from("/home/u/Downloads")
        );
        assert_eq!(
            downloads_dir(home, Some("XDG_DOWNLOAD_DIR=\"$HOME/Fetch\"\n")),
            PathBuf::from("/home/u/Fetch")
        );
        assert_eq!(
            downloads_dir(home, Some("XDG_DOWNLOAD_DIR=\"/data/dl\"\n")),
            PathBuf::from("/data/dl")
        );
        assert_eq!(
            downloads_dir(home, Some("XDG_DOWNLOAD_DIR=\"$(touch /tmp/x)\"\n")),
            PathBuf::from("/home/u/Downloads")
        );
    }

    #[test]
    fn notice_is_claimed_exactly_once_and_round_trips() {
        let mut latch = NoticeLatch::default();
        assert!(!latch.shown());
        assert!(latch.claim());
        assert!(!latch.claim());
        let persisted = NoticeLatch::parse(&latch.serialize());
        assert!(persisted.shown());
        assert!(!NoticeLatch::parse("").shown());
        assert!(!NoticeLatch::parse("shown=0\n").shown());
    }

    #[test]
    fn notice_text_is_exact() {
        assert_eq!(
            NOTICE_TEXT,
            "FlameWM organizes your downloads automatically into category folders. You can disable this in the settings."
        );
    }

    #[test]
    fn move_creates_folder_reuses_it_and_never_overwrites() {
        let root = unique_test_root("downloads-move");
        fs::create_dir_all(root.join("Documents")).unwrap();
        fs::write(root.join("Documents/a.pdf"), b"old").unwrap();
        fs::write(root.join("a.pdf"), b"new").unwrap();
        let moved = move_into_category(&root, "a.pdf", Category::Documents).unwrap();
        assert_eq!(moved, root.join("Documents/a (1).pdf"));
        assert_eq!(fs::read(root.join("Documents/a.pdf")).unwrap(), b"old");
        assert_eq!(fs::read(&moved).unwrap(), b"new");
        assert!(!root.join("a.pdf").exists());

        fs::write(root.join("p.png"), b"img").unwrap();
        let moved = move_into_category(&root, "p.png", Category::Multimedia).unwrap();
        assert_eq!(moved, root.join("Multimedia/p.png"));
        let _ = fs::remove_dir_all(&root);
    }
}
