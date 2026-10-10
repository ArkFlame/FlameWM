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
    file.write_all(text.as_bytes())
        .map_err(|error| io_error(&format!("write {owner}"), error))?;
    file.sync_all()
        .map_err(|error| io_error(&format!("sync {owner}"), error))?;
    fs::rename(&temp, path).map_err(|error| io_error(&format!("commit {owner}"), error))?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub(crate) fn io_error(operation: &str, error: std::io::Error) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, format!("{operation}: {error}"))
}
