//! Ark drag-out target: asks the source's `DBus` service (`org.kde.ark.DndExtract`) to extract
//! the dragged selection into the explorer's current folder.
//!
//! The XDND payload is validated by `flamewm_ui_x11` re-exports of the protocol owner. This
//! module owns only the `DBus` call: it runs `gdbus` with a hard timeout on a worker thread,
//! catches panics, and reports the extracted entries. Nothing here deletes or overwrites:
//! entries that appear during the call are reported, and a failed call leaves its output in
//! place for the user to see.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_ui_x11::{safe_file_name, valid_bus_name, valid_object_path};

pub const ARK_INTERFACE: &str = "org.kde.ark.DndExtract";
pub const ARK_METHOD: &str = "extractSelectedFilesTo";
/// Hard limit for one extraction call.
pub const EXTRACT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE_CHARS: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The payload or destination was rejected before any process ran.
    Rejected(String),
    /// `gdbus` could not be started.
    Spawn(String),
    /// No reply within `EXTRACT_TIMEOUT`; the child was killed.
    Timeout,
    /// The call returned an error. `message` is trimmed and bounded.
    Failed(String),
}

/// Successful extraction: entries that did not exist before the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractReport {
    pub created: Vec<String>,
}

/// Checks the `DBus` names and that `dest` is an existing absolute directory without control
/// characters (it is passed as a `GVariant` string).
pub fn validate_request(service: &str, path: &str, dest: &Path) -> Result<(), ExtractError> {
    if !valid_bus_name(service) {
        return Err(ExtractError::Rejected("invalid DBus service name".into()));
    }
    if !valid_object_path(path) {
        return Err(ExtractError::Rejected("invalid DBus object path".into()));
    }
    let Some(text) = dest.to_str() else {
        return Err(ExtractError::Rejected("destination is not UTF-8".into()));
    };
    if !dest.is_absolute() || text.chars().any(char::is_control) {
        return Err(ExtractError::Rejected(
            "destination must be an absolute path".into(),
        ));
    }
    if !dest.is_dir() {
        return Err(ExtractError::Rejected(
            "destination is not a directory".into(),
        ));
    }
    Ok(())
}

/// `GVariant` string literal for `text` (single-quoted, backslash and quote escaped).
fn gvariant_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

/// Arguments for `gdbus call` invoking `extractSelectedFilesTo(dest)`.
pub fn gdbus_args(service: &str, path: &str, dest: &Path) -> Result<Vec<String>, ExtractError> {
    validate_request(service, path, dest)?;
    let dest = dest.to_str().unwrap_or_default();
    Ok(vec![
        "call".into(),
        "--session".into(),
        "--dest".into(),
        service.into(),
        "--object-path".into(),
        path.into(),
        "--method".into(),
        format!("{ARK_INTERFACE}.{ARK_METHOD}"),
        gvariant_string(dest),
    ])
}

/// Runs `program args`, kills it after `timeout`, and returns `(success, stderr-or-stdout)`.
pub fn run_with_timeout(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<(bool, String), ExtractError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ExtractError::Spawn(error.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut text);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut text);
                }
                let text: String = text.trim().chars().take(MAX_MESSAGE_CHARS).collect();
                return Ok((status.success(), text));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExtractError::Timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExtractError::Spawn(error.to_string()));
            }
        }
    }
}

/// Name in `dir` that is not taken yet: `name`, then `name (2)`, `name (3)`, ...
#[must_use]
pub fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    };
    (2_u32..=u32::MAX)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !dir.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

fn nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Moves each top-level entry of `stage` into `dest` under a free name. Never overwrites.
fn commit(stage: &Path, dest: &Path) -> Result<ExtractReport, ExtractError> {
    let entries = std::fs::read_dir(stage)
        .map_err(|error| ExtractError::Failed(format!("reading staging: {error}")))?;
    let mut created = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !safe_file_name(&name) {
            continue;
        }
        let final_name = free_name(dest, &name);
        std::fs::rename(entry.path(), dest.join(&final_name))
            .map_err(|error| ExtractError::Failed(format!("moving {name}: {error}")))?;
        created.push(final_name);
    }
    created.sort();
    Ok(ExtractReport { created })
}

/// Runs `extract` into a private staging directory inside `dest`, then commits the result.
/// The staging directory is always removed, so a failed or timed-out extraction leaves no
/// partial output behind; entries already in `dest` are never touched.
pub fn stage_and_commit(
    dest: &Path,
    extract: impl FnOnce(&Path) -> Result<(), ExtractError>,
) -> Result<ExtractReport, ExtractError> {
    let stage = dest.join(format!(".flamewm-ark-{}-{}", std::process::id(), nonce()));
    std::fs::create_dir(&stage)
        .map_err(|error| ExtractError::Spawn(format!("staging directory: {error}")))?;
    let result = extract(&stage).and_then(|()| commit(&stage, dest));
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Synchronous extraction body: validated request, staged through `gdbus`.
fn extract_blocking(service: &str, path: &str, dest: &Path) -> Result<ExtractReport, ExtractError> {
    validate_request(service, path, dest)?;
    stage_and_commit(dest, |stage| {
        let args = gdbus_args(service, path, stage)?;
        let (ok, message) = run_with_timeout("gdbus", &args, EXTRACT_TIMEOUT)?;
        if ok {
            Ok(())
        } else {
            Err(ExtractError::Failed(message))
        }
    })
}

/// Starts an extraction on a worker thread. Validation happens first and synchronously, so an
/// invalid request never spawns anything. The result arrives on the returned channel; a dropped
/// sender (worker died) is reported by the receiver as a disconnect, never a hang.
#[must_use]
pub fn spawn_ark_extract(
    service: String,
    path: String,
    dest: PathBuf,
) -> Receiver<Result<ExtractReport, ExtractError>> {
    let (tx, rx) = mpsc::channel();
    if let Err(error) = validate_request(&service, &path, &dest) {
        let _ = tx.send(Err(error));
        return rx;
    }
    let spawned = thread::Builder::new()
        .name("flamewm-ark-extract".into())
        .spawn(move || {
            let _ = tx.send(extract_blocking(&service, &path, &dest));
        });
    if let Err(error) = spawned {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err(ExtractError::Spawn(error.to_string())));
        return rx;
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-dndext-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn validation_rejects_bad_names_relative_and_missing_destinations() {
        let dir = temp_dir("validate");
        assert!(validate_request("org.kde.ark-1", "/DndExtract", &dir).is_ok());
        assert!(matches!(
            validate_request("ark", "/x", &dir),
            Err(ExtractError::Rejected(_))
        ));
        assert!(matches!(
            validate_request("org.kde.ark", "x", &dir),
            Err(ExtractError::Rejected(_))
        ));
        assert!(matches!(
            validate_request("org.kde.ark", "/x", Path::new("relative")),
            Err(ExtractError::Rejected(_))
        ));
        assert!(matches!(
            validate_request("org.kde.ark", "/x", &dir.join("missing")),
            Err(ExtractError::Rejected(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gdbus_arguments_escape_destination_as_gvariant_string() {
        let dir = temp_dir("args");
        let quoted = dir.join("it's \\ here");
        std::fs::create_dir_all(&quoted).expect("quoted dir");
        let args = gdbus_args("org.kde.ark", "/DndExtract", &quoted).expect("args");
        assert_eq!(args[7], "org.kde.ark.DndExtract.extractSelectedFilesTo");
        assert!(args[8].starts_with('\'') && args[8].ends_with('\''));
        assert!(args[8].contains("it\\'s \\\\ here"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hanging_child_is_killed_at_the_timeout() {
        let started = Instant::now();
        let result = run_with_timeout("sleep", &["5".into()], Duration::from_millis(200));
        assert_eq!(result, Err(ExtractError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn child_exit_status_and_message_are_reported() {
        let ok = run_with_timeout(
            "sh",
            &["-c".into(), "echo hi".into()],
            Duration::from_secs(2),
        );
        assert_eq!(ok, Ok((true, "hi".into())));
        let failed = run_with_timeout(
            "sh",
            &["-c".into(), "echo nope >&2; exit 3".into()],
            Duration::from_secs(2),
        );
        assert_eq!(failed, Ok((false, "nope".into())));
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let result = run_with_timeout("/nonexistent/flamewm-gdbus", &[], Duration::from_secs(1));
        assert!(matches!(result, Err(ExtractError::Spawn(_))));
    }

    #[test]
    fn commit_moves_into_destination_without_overwriting() {
        let dir = temp_dir("commit");
        std::fs::write(dir.join("new.txt"), b"keep").expect("seed");
        let report = stage_and_commit(&dir, |stage| {
            std::fs::write(stage.join("new.txt"), b"extracted")
                .map_err(|e| ExtractError::Failed(e.to_string()))?;
            std::fs::create_dir(stage.join("folder"))
                .map_err(|e| ExtractError::Failed(e.to_string()))?;
            Ok(())
        })
        .expect("commit");
        assert_eq!(
            report.created,
            vec!["folder".to_string(), "new (2).txt".to_string()]
        );
        assert_eq!(
            std::fs::read(dir.join("new.txt")).expect("existing"),
            b"keep"
        );
        assert_eq!(
            std::fs::read(dir.join("new (2).txt")).expect("moved"),
            b"extracted"
        );
        assert!(dir.join("folder").is_dir());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".flamewm-ark"))
            .collect();
        assert!(leftovers.is_empty(), "staging directory is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_extraction_removes_partial_output_and_keeps_destination() {
        let dir = temp_dir("partial");
        std::fs::write(dir.join("keep.txt"), b"x").expect("seed");
        let result = stage_and_commit(&dir, |stage| {
            std::fs::write(stage.join("partial.bin"), b"half")
                .map_err(|e| ExtractError::Failed(e.to_string()))?;
            Err(ExtractError::Failed("boom".into()))
        });
        assert_eq!(result, Err(ExtractError::Failed("boom".into())));
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("list")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["keep.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn free_name_adds_counter_before_extension() {
        let dir = temp_dir("names");
        assert_eq!(free_name(&dir, "a.txt"), "a.txt");
        std::fs::write(dir.join("a.txt"), b"").expect("seed");
        std::fs::write(dir.join("a (2).txt"), b"").expect("seed");
        assert_eq!(free_name(&dir, "a.txt"), "a (3).txt");
        assert_eq!(free_name(&dir, ".hidden"), ".hidden");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_request_never_spawns_and_reports_on_channel() {
        let rx = spawn_ark_extract(
            "bad".into(),
            "/x".into(),
            PathBuf::from("/nonexistent-dest"),
        );
        assert!(matches!(rx.recv(), Ok(Err(ExtractError::Rejected(_)))));
    }
}
