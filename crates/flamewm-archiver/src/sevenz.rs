//! 7z through the system `7zz` or `7z` binary. No 7z decoder is linked.
//!
//! Listing parses `-slt` output. Extraction validates every listed name first, then unpacks into a
//! private staging directory inside the destination, removes symlinks and special nodes found there,
//! and moves the wanted top-level items into place. Creation writes a `.partial` file and renames it
//! on success. Cancel kills the child process through its handle.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::create::collect;
use crate::error::{ArchiveError, Result};
use crate::progress::{CancelToken, Progress, Summary};
use crate::safe_path;
use crate::visit::{Entry, Kind};
use crate::writer::partial_path;

const TOOL_NAMES: [&str; 2] = ["7zz", "7z"];
const POLL: Duration = Duration::from_millis(25);

/// Finds the first 7z binary on `PATH`. A missing tool is a clear error, never a silent fallback.
fn find_tool() -> Result<PathBuf> {
    for name in TOOL_NAMES {
        let probe = Command::new(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if probe.is_ok() {
            return Ok(PathBuf::from(name));
        }
    }
    Err(ArchiveError::Unsupported(
        "7z archives need 7zz or 7z on PATH".to_owned(),
    ))
}

/// Runs the tool to completion, polling `cancel`. Returns stdout; a non-zero exit is an error.
fn run(tool: &Path, args: &[OsString], cwd: Option<&Path>, cancel: &CancelToken) -> Result<String> {
    let mut cmd = Command::new(tool);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    loop {
        if let Some(status) = child.try_wait()? {
            let out = stdout.and_then(|h| h.join().ok()).unwrap_or_default();
            if status.success() {
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            let err = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
            return Err(ArchiveError::Io(io::Error::other(format!(
                "7z exited with {status}: {}",
                String::from_utf8_lossy(&err).trim()
            ))));
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ArchiveError::Cancelled);
        }
        thread::sleep(POLL);
    }
}

/// Reads a child pipe on its own thread so a full pipe can never stall the child.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// Lists the entries of a 7z archive.
pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    list_with(&find_tool()?, archive, &CancelToken::new())
}

fn list_with(tool: &Path, archive: &Path, cancel: &CancelToken) -> Result<Vec<Entry>> {
    let args = [
        OsString::from("l"),
        OsString::from("-slt"),
        OsString::from("--"),
        archive.as_os_str().to_owned(),
    ];
    Ok(parse_listing(&run(tool, &args, None, cancel)?))
}

/// Parses `7z l -slt` output. Only the blocks after the separator line are entries.
pub fn parse_listing(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut in_entries = false;
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if !in_entries {
            in_entries = line.trim_end() == "----------";
            continue;
        }
        if line.trim().is_empty() {
            if let Some(entry) = entry_from_block(&block) {
                entries.push(entry);
            }
            block.clear();
        } else {
            block.push(line);
        }
    }
    entries
}

fn entry_from_block(block: &[&str]) -> Option<Entry> {
    let mut path = None;
    let mut size = 0u64;
    let mut attributes = "";
    let mut folder = false;
    for line in block {
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        match key {
            "Path" => path = Some(value),
            "Size" => size = value.parse().unwrap_or(0),
            "Attributes" => attributes = value,
            "Folder" => folder = value == "+",
            _ => {}
        }
    }
    let unix = attributes
        .split_whitespace()
        .last()
        .filter(|token| token.len() == 10 && token.is_ascii());
    let windows_dir = attributes
        .split_whitespace()
        .next()
        .is_some_and(|flags| flags.contains('D'));
    let kind = match unix.map(|token| token.as_bytes()[0]) {
        _ if folder || windows_dir => Kind::Dir,
        Some(b'd') => Kind::Dir,
        Some(b'l') => Kind::Symlink,
        Some(b'-') | None => Kind::File,
        Some(_) => Kind::Other,
    };
    Some(Entry {
        path: PathBuf::from(path?),
        size,
        kind,
        mode: unix.map(unix_bits),
        encrypted: false,
    })
}

/// Permission bits from a `drwxr-xr-x` string. Set-id and sticky marks count as their base bit.
fn unix_bits(text: &str) -> u32 {
    text.bytes()
        .skip(1)
        .enumerate()
        .filter(|(_, ch)| *ch != b'-')
        .fold(0u32, |bits, (i, _)| bits | (0o400 >> i))
}

/// Extracts a 7z archive. `selection` filters the top-level items moved into `dest`.
pub fn extract(
    archive: &Path,
    dest: &Path,
    selection: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    extract_with(&find_tool()?, archive, dest, selection, cancel, sink)
}

pub(crate) fn extract_with(
    tool: &Path,
    archive: &Path,
    dest: &Path,
    selection: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let entries = list_with(tool, archive, cancel)?;
    // Every listed name is checked before the tool runs, so a hostile name writes nothing.
    for entry in &entries {
        safe_path::sanitize(&entry.path)?;
    }
    let roots = selection
        .iter()
        .map(|root| safe_path::sanitize(root))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !selection.is_empty() && roots.is_empty() {
        return Ok(Summary::default());
    }
    let progress = Totals {
        bytes: entries
            .iter()
            .filter(|e| e.kind == Kind::File)
            .map(|e| e.size)
            .sum(),
        entries: entries.len(),
    };

    let created = !dest.exists();
    safe_path::reject_symlinks(dest, Path::new(""))?;
    fs::create_dir_all(dest)?;
    let stage = dest.join(format!(".flamewm-7z-stage-{}", std::process::id()));
    let result = stage_and_move(tool, archive, dest, &stage, &roots, progress, cancel, sink);
    let _ = fs::remove_dir_all(&stage);
    match result {
        Ok(summary) => Ok(summary),
        Err(err) => {
            if created {
                let _ = fs::remove_dir_all(dest);
            }
            Err(err)
        }
    }
}

#[derive(Clone, Copy)]
struct Totals {
    bytes: u64,
    entries: usize,
}

#[allow(clippy::too_many_arguments)]
fn stage_and_move(
    tool: &Path,
    archive: &Path,
    dest: &Path,
    stage: &Path,
    roots: &[PathBuf],
    totals: Totals,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    fs::create_dir(stage)?;
    let mut out_flag = OsString::from("-o");
    out_flag.push(stage);
    let args = [
        OsString::from("x"),
        OsString::from("-y"),
        OsString::from("-bd"),
        OsString::from("-bso0"),
        OsString::from("-bsp0"),
        out_flag,
        OsString::from("--"),
        archive.as_os_str().to_owned(),
    ];
    run(tool, &args, None, cancel)?;

    let mut skipped = 0usize;
    prune(stage, &mut skipped)?;
    let items = if roots.is_empty() {
        fs::read_dir(stage)?
            .map(|entry| entry.map(|e| PathBuf::from(e.file_name())))
            .collect::<io::Result<Vec<_>>>()?
    } else {
        roots.to_vec()
    };

    // Check every target before the first move so a clash leaves the destination untouched.
    let mut pairs = Vec::new();
    for rel in items {
        let src = stage.join(&rel);
        if fs::symlink_metadata(&src).is_err() {
            continue;
        }
        let target = dest.join(&rel);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", target.display()),
            )
            .into());
        }
        safe_path::reject_symlinks(dest, &rel)?;
        pairs.push((src, target));
    }

    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    for (src, target) in pairs {
        if cancel.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&src, &target)?;
        tally(&target, &mut summary)?;
        sink(Progress {
            bytes_done: summary.bytes,
            bytes_total: totals.bytes,
            entries_done: summary.files + summary.dirs,
            entries_total: totals.entries,
        });
    }
    Ok(summary)
}

/// Removes symlinks and special nodes from the staging tree. Regular files and directories stay.
fn prune(dir: &Path, skipped: &mut usize) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_dir() {
            prune(&path, skipped)?;
        } else if !file_type.is_file() {
            fs::remove_file(&path)?;
            *skipped += 1;
        }
    }
    Ok(())
}

/// Adds the counts of a moved tree to `summary`.
fn tally(path: &Path, summary: &mut Summary) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        summary.dirs += 1;
        for child in fs::read_dir(path)? {
            tally(&child?.path(), summary)?;
        }
    } else {
        summary.files += 1;
        summary.bytes += meta.len();
    }
    Ok(())
}

/// Creates a 7z archive from `inputs`, which must share one parent folder.
pub fn create(
    out: &Path,
    inputs: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    create_with(&find_tool()?, out, inputs, cancel, sink)
}

fn create_with(
    tool: &Path,
    out: &Path,
    inputs: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let (sources, skipped) = collect(inputs)?;
    let folder_of = |input: &Path| {
        input
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let parent = inputs
        .first()
        .map_or_else(|| PathBuf::from("."), |i| folder_of(i));
    if inputs.iter().any(|input| folder_of(input) != parent) {
        return Err(ArchiveError::Unsupported(
            "7z create needs all inputs in one folder".to_owned(),
        ));
    }
    let names = inputs
        .iter()
        .map(|input| {
            input
                .file_name()
                .map(|name| name.to_owned())
                .ok_or_else(|| ArchiveError::Unsupported(input.display().to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    // 7z updates an existing archive of the same name, so a stale partial is removed first.
    let out_abs = std::path::absolute(out)?;
    let partial = partial_path(&out_abs);
    let _ = fs::remove_file(&partial);
    let mut args = vec![
        OsString::from("a"),
        OsString::from("-t7z"),
        OsString::from("-y"),
        OsString::from("-bd"),
        OsString::from("-bso0"),
        OsString::from("-bsp0"),
        OsString::from("-snl"),
        OsString::from("--"),
        partial.as_os_str().to_owned(),
    ];
    args.extend(names);
    if let Err(err) = run(tool, &args, Some(&parent), cancel) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    fs::rename(&partial, &out_abs)?;

    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    for source in &sources {
        if source.is_dir {
            summary.dirs += 1;
        } else {
            summary.files += 1;
            summary.bytes += source.size;
        }
    }
    sink(Progress {
        bytes_done: summary.bytes,
        bytes_total: summary.bytes,
        entries_done: sources.len(),
        entries_total: sources.len(),
    });
    Ok(summary)
}
