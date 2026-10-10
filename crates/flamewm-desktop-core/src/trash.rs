use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::atomic_file::io_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashScope {
    Home,
    TopDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashLocation {
    pub scope: TrashScope,
    pub top_directory: Option<PathBuf>,
    pub base: PathBuf,
}

impl TrashLocation {
    #[must_use]
    pub fn files_dir(&self) -> PathBuf {
        self.base.join("files")
    }

    #[must_use]
    pub fn info_dir(&self) -> PathBuf {
        self.base.join("info")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedItem {
    pub original: PathBuf,
    pub stored: PathBuf,
    pub info: PathBuf,
}

pub struct Trash;

impl Trash {
    /// Select a Freedesktop-compliant location. Files on the home filesystem use
    /// `$XDG_DATA_HOME/Trash`; other files use the mount top-directory trash when possible.
    pub fn location_for(
        path: &Path,
        home: &Path,
        xdg_data_home: &Path,
        uid: u32,
    ) -> FlameResult<TrashLocation> {
        let target_meta =
            fs::symlink_metadata(path).map_err(|error| io_error("stat target", error))?;
        let home_meta = fs::metadata(home).map_err(|error| io_error("stat home", error))?;
        if target_meta.dev() == home_meta.dev() {
            return Ok(TrashLocation {
                scope: TrashScope::Home,
                top_directory: None,
                base: xdg_data_home.join("Trash"),
            });
        }

        let top = filesystem_top(path, target_meta.dev())?;
        let shared = top.join(".Trash");
        if is_valid_shared_trash(&shared) {
            return Ok(TrashLocation {
                scope: TrashScope::TopDirectory,
                top_directory: Some(top),
                base: shared.join(uid.to_string()),
            });
        }
        Ok(TrashLocation {
            scope: TrashScope::TopDirectory,
            top_directory: Some(top.clone()),
            base: top.join(format!(".Trash-{uid}")),
        })
    }

    pub fn move_to_trash(
        path: &Path,
        home: &Path,
        xdg_data_home: &Path,
        uid: u32,
    ) -> FlameResult<TrashedItem> {
        if !path.is_absolute() {
            return Err(FlameError::invalid("trash source path must be absolute"));
        }
        let location = Self::location_for(path, home, xdg_data_home, uid)?;
        ensure_location(&location)?;

        let original_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("item");
        let info_path_value = info_path_for(path, &location)?;
        let content = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            percent_encode_path(&info_path_value),
            deletion_date_utc(SystemTime::now())
        );

        // `reserve_unique` atomically claims the `.trashinfo` name, so the file is already open.
        let (stored, info, mut info_file) = reserve_unique(&location, original_name)?;
        if let Err(error) = info_file
            .write_all(content.as_bytes())
            .and_then(|()| info_file.sync_all())
        {
            let _ = fs::remove_file(&info);
            return Err(io_error("write .trashinfo", error));
        }

        if let Err(error) = rename_no_replace(path, &stored) {
            let _ = fs::remove_file(&info);
            return Err(error);
        }
        Ok(TrashedItem {
            original: path.to_path_buf(),
            stored,
            info,
        })
    }

    pub fn empty(location: &TrashLocation) -> FlameResult<usize> {
        let mut removed = 0_usize;
        if let Ok(entries) = fs::read_dir(location.files_dir()) {
            for entry in entries {
                let entry = entry.map_err(|error| io_error("read Trash file", error))?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| io_error("stat Trash file", error))?;
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    fs::remove_dir_all(&path)
                        .map_err(|error| io_error("remove trashed directory", error))?;
                } else {
                    fs::remove_file(&path)
                        .map_err(|error| io_error("remove trashed file", error))?;
                }
                removed = removed.saturating_add(1);
            }
        }
        if let Ok(entries) = fs::read_dir(location.info_dir()) {
            for entry in entries {
                let path = entry
                    .map_err(|error| io_error("read Trash info", error))?
                    .path();
                fs::remove_file(&path).map_err(|error| io_error("remove Trash info", error))?;
            }
        }
        Ok(removed)
    }
}

#[must_use]
pub fn current_uid_from_proc(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        let values = line.strip_prefix("Uid:")?;
        values.split_whitespace().next()?.parse().ok()
    })
}

fn filesystem_top(path: &Path, device: u64) -> FlameResult<PathBuf> {
    let mut current = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .ok_or_else(|| FlameError::invalid("trash source has no parent"))?
            .to_path_buf()
    };
    loop {
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        let parent_meta =
            fs::metadata(parent).map_err(|error| io_error("stat mount parent", error))?;
        if parent_meta.dev() != device {
            break;
        }
        current = parent.to_path_buf();
    }
    Ok(current)
}

fn is_valid_shared_trash(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o1000 != 0
}

fn ensure_location(location: &TrashLocation) -> FlameResult<()> {
    if location.scope == TrashScope::TopDirectory {
        if let Some(top) = location.top_directory.as_ref() {
            if location.base
                == top
                    .join(".Trash")
                    .join(location.base.file_name().unwrap_or_default())
            {
                if !is_valid_shared_trash(&top.join(".Trash")) {
                    return Err(FlameError::new(
                        ErrorCode::PermissionDenied,
                        "shared .Trash is unsafe",
                    ));
                }
            }
        }
    }
    ensure_private_dir(&location.base)?;
    ensure_private_dir(&location.files_dir())?;
    ensure_private_dir(&location.info_dir())?;
    Ok(())
}

fn ensure_private_dir(path: &Path) -> FlameResult<()> {
    if path.exists() {
        let metadata =
            fs::symlink_metadata(path).map_err(|error| io_error("stat Trash directory", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(FlameError::new(
                ErrorCode::PermissionDenied,
                "Trash path is not a safe directory",
            ));
        }
    } else {
        fs::create_dir_all(path).map_err(|error| io_error("create Trash directory", error))?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("chmod Trash directory", error))?;
    Ok(())
}

/// Claims a unique Trash name. The `.trashinfo` file is created with `create_new`
/// (atomic), and the stored name is checked with `symlink_metadata` so dangling
/// symlinks count as occupied. The returned file handle is the claimed info file.
fn reserve_unique(
    location: &TrashLocation,
    name: &str,
) -> FlameResult<(PathBuf, PathBuf, fs::File)> {
    for sequence in 0_u32..10_000 {
        let candidate = if sequence == 0 {
            name.to_owned()
        } else {
            format!("{name}.{sequence}")
        };
        let stored = location.files_dir().join(&candidate);
        let info = location.info_dir().join(format!("{candidate}.trashinfo"));
        let info_file = match OpenOptions::new().create_new(true).write(true).open(&info) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("create .trashinfo", error)),
        };
        if fs::symlink_metadata(&stored).is_ok() {
            drop(info_file);
            let _ = fs::remove_file(&info);
            continue;
        }
        return Ok((stored, info, info_file));
    }
    Err(FlameError::new(
        ErrorCode::Conflict,
        "unable to allocate unique Trash name",
    ))
}

/// Move `source` to `target` without replacing an existing entry (dangling symlinks included).
fn rename_no_replace(source: &Path, target: &Path) -> FlameResult<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        target,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| match error {
        rustix::io::Errno::EXIST => {
            FlameError::new(ErrorCode::Conflict, "Trash entry already exists")
        }
        rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP => {
            FlameError::new(ErrorCode::Unsupported, "atomic rename not supported")
        }
        other => FlameError::new(
            ErrorCode::IoFailure,
            format!("move item into Trash: {other}"),
        ),
    })
}

fn info_path_for(path: &Path, location: &TrashLocation) -> FlameResult<PathBuf> {
    match location.scope {
        TrashScope::Home => Ok(path.to_path_buf()),
        TrashScope::TopDirectory => {
            let top = location.top_directory.as_ref().ok_or_else(|| {
                FlameError::new(ErrorCode::InternalFailure, "missing trash top directory")
            })?;
            path.strip_prefix(top).map(Path::to_path_buf).map_err(|_| {
                FlameError::new(
                    ErrorCode::InternalFailure,
                    "path not below trash top directory",
                )
            })
        }
    }
}

#[must_use]
pub fn percent_encode_path(path: &Path) -> String {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut out = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[must_use]
pub fn deletion_date_utc(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0_i64, |duration| {
        duration.as_secs().min(i64::MAX as u64) as i64
    });
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

// Howard Hinnant's civil-from-days transformation, with the Unix epoch offset.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_info_path_percent_encodes_spaces_without_shell_semantics() {
        assert_eq!(
            percent_encode_path(Path::new("/home/juan/My File.txt")),
            "/home/juan/My%20File.txt"
        );
    }

    #[test]
    fn epoch_has_canonical_trash_info_timestamp_shape() {
        assert_eq!(deletion_date_utc(UNIX_EPOCH), "1970-01-01T00:00:00");
    }

    #[test]
    fn uid_parser_uses_real_uid_column() {
        assert_eq!(
            current_uid_from_proc("Name:\tx\nUid:\t1000\t1000\t1000\t1000\n"),
            Some(1000)
        );
    }

    /// Removes the fixture tree on drop so the test cleans up even on failure.
    struct TempTree(PathBuf);

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unique_temp_root(label: &str) -> TempTree {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "flamewm-trash-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        TempTree(root)
    }

    #[test]
    fn trash_does_not_reuse_name_held_by_dangling_symlink() {
        let tree = unique_temp_root("dangling");
        let root = &tree.0;
        let home = root.join("home");
        let xdg = root.join("xdg");
        let work = root.join("home").join("work");
        let files = xdg.join("Trash").join("files");
        let info = xdg.join("Trash").join("info");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&files).unwrap();
        fs::create_dir_all(&info).unwrap();

        // Dangling symlink already occupies the Trash name `foo`.
        std::os::unix::fs::symlink(root.join("missing-target"), files.join("foo")).unwrap();

        let source = work.join("foo");
        fs::write(&source, b"data").unwrap();

        let item = Trash::move_to_trash(&source, &home, &xdg, 1000).unwrap();

        assert_eq!(item.stored, files.join("foo.1"));
        assert!(
            fs::symlink_metadata(files.join("foo"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "pre-existing dangling symlink must survive the trash move"
        );
        assert_eq!(fs::read(files.join("foo.1")).unwrap(), b"data");
    }
}
