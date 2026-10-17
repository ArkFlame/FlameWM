use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use flamewm_api::{ErrorCode, FlameError, FlameResult};

pub(crate) fn write_text(path: &Path, owner: &str, text: &str) -> FlameResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| FlameError::invalid(format!("{owner} path has no parent")))?;
    fs::create_dir_all(parent)
        .map_err(|error| io_error(&format!("create {owner} directory"), error))?;
    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| io_error(&format!("open {owner} temp file"), error))?;
    let committed = file
        .write_all(text.as_bytes())
        .map_err(|error| io_error(&format!("write {owner}"), error))
        .and_then(|()| {
            file.sync_all()
                .map_err(|error| io_error(&format!("sync {owner}"), error))
        })
        .and_then(|()| {
            fs::rename(&temp, path).map_err(|error| io_error(&format!("commit {owner}"), error))
        });
    if let Err(error) = committed {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub(crate) fn io_error(operation: &str, error: std::io::Error) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, format!("{operation}: {error}"))
}

#[cfg(test)]
pub(crate) fn unique_test_root(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_removes_temp_file_when_commit_fails() {
        let root = unique_test_root("flamewm-atomic");
        let target = root.join("layout.txt");
        // A directory at the target makes rename(file, dir) fail with EISDIR after the temp file exists.
        fs::create_dir_all(&target).unwrap();
        let error =
            write_text(&target, "test owner", "payload").expect_err("commit onto directory fails");
        assert_eq!(error.code, ErrorCode::IoFailure);
        assert!(
            error.message.starts_with("commit test owner:"),
            "{}",
            error.message
        );
        assert!(!root.join("layout.txt.tmp").exists(), "temp file leaked");
        let _ = fs::remove_dir_all(&root);
    }
}
