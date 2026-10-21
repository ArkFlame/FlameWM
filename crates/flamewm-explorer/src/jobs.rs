//! File-operation job owner: cancellable crash-safe copy, no-replace publish,
//! verified cross-device moves, conflict policy and the undo journal.
//! All filesystem I/O runs on the worker thread; the UI only sends requests.
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{renameat_with, RenameFlags, CWD};
use rustix::io::Errno;

use crate::{destination_for, validate_transfer_destination, BatchReport};

const CHUNK: usize = 65_536;
/// Guards against huge or deeply nested trees while counting on the worker.
pub const MAX_COUNT_ITEMS: u64 = 1_000_000;
pub const MAX_COUNT_DEPTH: usize = 128;
const MAX_UNDO: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub u64);
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
impl JobId {
    pub fn next() -> Self { JobId(NEXT_JOB.fetch_add(1, Ordering::Relaxed)) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp { Copy, Move, Trash, Rename, Duplicate, NewFolder }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState { Queued, Counting, Running, Cancelling, Completed, PartiallyFailed, Failed }

/// Shared cancellation flag; cloning shares the same flag.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn new() -> Self { Self::default() }
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals { pub items: u64, pub bytes: u64, pub errors: u64 }

/// Determinate progress fields for one job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_items: u64,
    pub total_items: u64,
    pub error_count: u64,
}

/// What to do when the destination name already exists. Never overwrites
/// unless `Replace` is chosen explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conflict {
    #[default]
    KeepBoth,
    Skip,
    Replace,
    Cancel,
}

/// Reversible operations recorded after success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    Moved { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    Trashed { original: PathBuf, stored: PathBuf, info: PathBuf },
}

#[derive(Debug, Default)]
pub struct Journal { entries: Vec<Undo> }
impl Journal {
    pub fn record(&mut self, entries: impl IntoIterator<Item = Undo>) {
        self.entries.extend(entries);
        let excess = self.entries.len().saturating_sub(MAX_UNDO);
        self.entries.drain(..excess);
    }
    pub fn take_last(&mut self) -> Option<Undo> { self.entries.pop() }
    pub fn restore(&mut self, entry: Undo) { self.entries.push(entry); }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

pub fn journal() -> &'static Mutex<Journal> {
    static JOURNAL: OnceLock<Mutex<Journal>> = OnceLock::new();
    JOURNAL.get_or_init(|| Mutex::new(Journal::default()))
}

pub fn record(entries: impl IntoIterator<Item = Undo>) {
    if let Ok(mut journal) = journal().lock() { journal.record(entries); }
}

/// Reverse one journal entry. Every reversal refuses to overwrite.
pub fn undo(entry: &Undo) -> io::Result<()> {
    match entry {
        Undo::Moved { from, to } => rename_into(to, from, false),
        Undo::Created { path } => fs::remove_dir(path),
        Undo::Trashed { original, stored, info } => {
            rename_into(stored, original, false)?;
            let _ = fs::remove_file(info);
            Ok(())
        }
    }
}

fn cancelled() -> io::Error { io::Error::new(io::ErrorKind::Interrupted, "cancelled") }

fn rename_into(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let flags = if replace { RenameFlags::empty() } else { RenameFlags::NOREPLACE };
    renameat_with(CWD, from, CWD, to, flags).map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))
}

fn staging_path(to: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    to.with_file_name(format!(".flamewm-part-{}-{stamp}-{seq}-{name}", std::process::id()))
}

fn remove_entry(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Copies into a fresh staging entry. Symlinks are recreated, never followed.
/// Permissions are preserved; hardlinks become independent copies.
fn copy_tree(from: &Path, to: &Path, cancel: &CancellationToken, report: &mut impl FnMut(u64)) -> io::Result<()> {
    if cancel.is_cancelled() { return Err(cancelled()); }
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for child in fs::read_dir(from)? {
            let child = child?;
            copy_tree(&child.path(), &to.join(child.file_name()), cancel, report)?;
        }
        return fs::set_permissions(to, meta.permissions());
    }
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "not a regular file"));
    }
    let mut input = File::open(from)?;
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        if cancel.is_cancelled() { return Err(cancelled()); }
        let len = input.read(&mut buffer)?;
        if len == 0 { break; }
        output.write_all(&buffer[..len])?;
        report(len as u64);
    }
    output.sync_all()?;
    drop(output);
    fs::set_permissions(to, meta.permissions())
}

/// Atomically publish a staging entry. `replace=false` uses renameat2(NOREPLACE)
/// so a path created after the existence check is never overwritten.
fn publish(staging: &Path, to: &Path, replace: bool) -> io::Result<()> {
    rename_into(staging, to, replace)?;
    if let Some(parent) = to.parent() {
        let _ = File::open(parent).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

/// Copy `from` to `to` through a unique same-directory staging entry.
/// Staging is removed on every failure or cancellation.
pub fn copy_publish(from: &Path, to: &Path, replace: bool, cancel: &CancellationToken,
    report: &mut impl FnMut(u64)) -> io::Result<()> {
    if fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) {
        if to.starts_with(from) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a directory into itself"));
        }
        // Reject symlink aliases of descendants before creating output.
        if let (Ok(source), Some(parent)) = (fs::canonicalize(from), to.parent()) {
            if fs::canonicalize(parent).is_ok_and(|dp| dp.starts_with(&source)) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput,
                    "cannot copy a directory into itself through a symlink"));
            }
        }
    }
    let staging = staging_path(to);
    let result = copy_tree(from, &staging, cancel, report).and_then(|()| publish(&staging, to, replace));
    if result.is_err() {
        let _ = remove_entry(&staging);
    }
    result
}

/// Same-filesystem rename is atomic. Cross-device moves copy, verify the copy
/// against the source manifest, and only then delete the source.
pub fn move_publish(from: &Path, to: &Path, replace: bool, cancel: &CancellationToken,
    report: &mut impl FnMut(u64)) -> io::Result<()> {
    match rename_into(from, to, replace) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
            copy_publish(from, to, replace, cancel, report)?;
            let want = count_tree(from, &CancellationToken::default())?;
            let got = count_tree(to, &CancellationToken::default())?;
            if want.errors != 0 || got.errors != 0 || want != got {
                let _ = remove_entry(to);
                return Err(io::Error::other("copy verification failed; source kept"));
            }
            remove_entry(from).map_err(|e| io::Error::other(
                format!("copy verified but source removal failed: {e}")))
        }
        Err(error) => Err(error),
    }
}

/// Counts items and bytes without following symlinks. Unreadable entries are
/// counted as errors; cancellation and size/depth guards abort the count.
pub fn count_tree(path: &Path, cancel: &CancellationToken) -> io::Result<Totals> {
    let meta = fs::symlink_metadata(path)?;
    let mut totals = Totals::default();
    count_meta(path, &meta, 0, cancel, &mut totals)?;
    Ok(totals)
}

fn count_into(path: &Path, depth: usize, cancel: &CancellationToken, totals: &mut Totals) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => { totals.errors += 1; return Ok(()); }
    };
    count_meta(path, &meta, depth, cancel, totals)
}

fn count_meta(path: &Path, meta: &fs::Metadata, depth: usize, cancel: &CancellationToken,
    totals: &mut Totals) -> io::Result<()> {
    if cancel.is_cancelled() { return Err(cancelled()); }
    if totals.items >= MAX_COUNT_ITEMS || depth > MAX_COUNT_DEPTH {
        return Err(io::Error::other("tree too large or too deep to transfer"));
    }
    totals.items += 1;
    if meta.file_type().is_symlink() { return Ok(()); }
    if meta.is_dir() {
        match fs::read_dir(path) {
            Ok(children) => {
                for child in children {
                    match child {
                        Ok(child) => count_into(&child.path(), depth + 1, cancel, totals)?,
                        Err(_) => totals.errors += 1,
                    }
                }
            }
            Err(_) => totals.errors += 1,
        }
    } else if meta.is_file() {
        totals.bytes += meta.len();
    }
    Ok(())
}

/// Total of several top-level sources; a failed source counts as one error.
pub fn count_sources(sources: &[PathBuf], cancel: &CancellationToken) -> Totals {
    let mut sum = Totals::default();
    for source in sources {
        match count_tree(source, cancel) {
            Ok(t) => { sum.items += t.items; sum.bytes += t.bytes; sum.errors += t.errors; }
            Err(_) => sum.errors += 1,
        }
    }
    sum
}

enum Step { Done(Option<Undo>), Skipped }

fn transfer_one(source: &Path, directory: &Path, move_it: bool, conflict: Conflict,
    cancel: &CancellationToken, report: &mut impl FnMut(u64)) -> io::Result<Step> {
    if move_it && source.parent() == Some(directory) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "source already in destination folder"));
    }
    validate_transfer_destination(source, directory)?;
    let name = source.file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no file name"))?;
    let mut dest = directory.join(name);
    let mut replace = false;
    if fs::symlink_metadata(&dest).is_ok() {
        match conflict {
            Conflict::KeepBoth => dest = destination_for(source, directory),
            Conflict::Skip => return Ok(Step::Skipped),
            Conflict::Replace => replace = true,
            Conflict::Cancel => { cancel.cancel(); return Err(cancelled()); }
        }
    }
    if dest == source {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot overwrite source"));
    }
    if move_it {
        move_publish(source, &dest, replace, cancel, report)?;
        Ok(Step::Done(Some(Undo::Moved { from: source.to_path_buf(), to: dest })))
    } else {
        copy_publish(source, &dest, replace, cancel, report)?;
        Ok(Step::Done(None))
    }
}

/// Transfers every source, continuing past per-item errors. A cancelled batch
/// stops before the next item and leaves no staging entries behind.
pub fn transfer_batch_with(sources: &[PathBuf], directory: &Path, move_it: bool, conflict: Conflict,
    cancel: &CancellationToken, report: &mut impl FnMut(u64)) -> BatchReport {
    let mut outcome = BatchReport::default();
    for source in sources {
        if cancel.is_cancelled() { outcome.cancelled = true; break; }
        match transfer_one(source, directory, move_it, conflict, cancel, report) {
            Ok(Step::Done(undo)) => {
                outcome.completed += 1;
                outcome.undo.extend(undo);
            }
            Ok(Step::Skipped) => outcome.skipped += 1,
            Err(error) => {
                if cancel.is_cancelled() { outcome.cancelled = true; break; }
                outcome.record(source, Err(error));
            }
        }
    }
    outcome
}

/// Drop policy: move within one filesystem, copy across devices, Ctrl forces copy.
pub fn transfer_drop_batch_with(sources: &[PathBuf], directory: &Path, force_copy: bool,
    cancel: &CancellationToken, report: &mut impl FnMut(u64)) -> BatchReport {
    use std::os::unix::fs::MetadataExt;
    let destination_device = fs::metadata(directory).ok().map(|m| m.dev());
    let mut result = BatchReport::default();
    for source in sources {
        if cancel.is_cancelled() { result.cancelled = true; break; }
        let source_device = fs::symlink_metadata(source).ok().map(|m| m.dev());
        let should_move = source_device.zip(destination_device)
            .is_some_and(|(src, dst)| crate::drop_moves(force_copy, src, dst));
        result.append(transfer_batch_with(std::slice::from_ref(source), directory, should_move,
            Conflict::KeepBoth, cancel, report));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("flamewm-jobs-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
        fn path(&self) -> &Path { &self.0 }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            // Restore traversal permissions a test may have removed before cleanup.
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn staging_left(dir: &Path) -> usize {
        fs::read_dir(dir).unwrap().filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".flamewm-part-")).count()
    }

    #[test]
    fn nested_tree_copies_symlinks_without_following_and_counts_match() {
        let scratch = Scratch::new("tree");
        let src = scratch.path().join("a");
        fs::create_dir_all(src.join("b")).unwrap();
        fs::write(src.join("b/c.txt"), b"hello").unwrap();
        fs::set_permissions(src.join("b/c.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        symlink("b", src.join("link")).unwrap();
        symlink(".", src.join("loop")).unwrap();

        let cancel = CancellationToken::new();
        let totals = count_tree(&src, &cancel).unwrap();
        assert_eq!(totals, Totals { items: 5, bytes: 5, errors: 0 });

        let dest_dir = scratch.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        let outcome = transfer_batch_with(std::slice::from_ref(&src), &dest_dir, false,
            Conflict::KeepBoth, &cancel, &mut |_| {});
        assert_eq!((outcome.completed, outcome.failed), (1, 0), "{outcome:?}");
        let copied = dest_dir.join("a");
        assert_eq!(fs::read_link(copied.join("link")).unwrap(), PathBuf::from("b"));
        assert_eq!(fs::read_link(copied.join("loop")).unwrap(), PathBuf::from("."));
        assert_eq!(fs::read(copied.join("b/c.txt")).unwrap(), b"hello");
        assert_eq!(fs::metadata(copied.join("b/c.txt")).unwrap().mode() & 0o777, 0o640);
        assert_eq!(staging_left(&dest_dir), 0);
        assert_eq!(fs::read(src.join("b/c.txt")).unwrap(), b"hello");
    }

    #[test]
    fn collision_policies_never_overwrite_unless_replace_is_chosen() {
        let scratch = Scratch::new("collide");
        let src = scratch.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("x"), b"new").unwrap();
        let cancel = CancellationToken::new();
        let source = vec![src.join("x")];

        let keep = scratch.path().join("keep");
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("x"), b"old").unwrap();
        let outcome = transfer_batch_with(&source, &keep, false, Conflict::KeepBoth, &cancel, &mut |_| {});
        assert_eq!(outcome.completed, 1);
        assert_eq!(fs::read(keep.join("x")).unwrap(), b"old");
        assert_eq!(fs::read(keep.join("x (2)")).unwrap(), b"new");

        let skip = scratch.path().join("skip");
        fs::create_dir_all(&skip).unwrap();
        fs::write(skip.join("x"), b"old").unwrap();
        let outcome = transfer_batch_with(&source, &skip, false, Conflict::Skip, &cancel, &mut |_| {});
        assert_eq!((outcome.completed, outcome.skipped, outcome.failed), (0, 1, 0));
        assert_eq!(fs::read(skip.join("x")).unwrap(), b"old");

        let cancel_dir = scratch.path().join("cancel");
        fs::create_dir_all(&cancel_dir).unwrap();
        fs::write(cancel_dir.join("x"), b"old").unwrap();
        let outcome = transfer_batch_with(&source, &cancel_dir, false, Conflict::Cancel, &CancellationToken::new(), &mut |_| {});
        assert!(outcome.cancelled);
        assert_eq!(fs::read(cancel_dir.join("x")).unwrap(), b"old");
        assert_eq!(outcome.completed, 0);

        let replace = scratch.path().join("replace");
        fs::create_dir_all(&replace).unwrap();
        fs::write(replace.join("x"), b"old").unwrap();
        let outcome = transfer_batch_with(&source, &replace, false, Conflict::Replace, &CancellationToken::new(), &mut |_| {});
        assert_eq!(outcome.completed, 1);
        assert_eq!(fs::read(replace.join("x")).unwrap(), b"new");
    }

    #[test]
    fn cancel_mid_copy_stops_and_removes_staging() {
        let scratch = Scratch::new("cancel");
        let src = scratch.path().join("big.bin");
        fs::write(&src, vec![7_u8; 1024 * 1024]).unwrap();
        let dest_dir = scratch.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let mut report = |n: u64| { if n > 0 { trigger.cancel(); } };
        let outcome = transfer_batch_with(std::slice::from_ref(&src), &dest_dir, false,
            Conflict::KeepBoth, &cancel, &mut report);
        assert!(outcome.cancelled);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.completed, 0);
        assert!(!dest_dir.join("big.bin").exists());
        assert_eq!(staging_left(&dest_dir), 0);
        assert_eq!(fs::metadata(&src).unwrap().len(), 1024 * 1024);
    }

    #[test]
    fn partial_failure_reports_error_and_keeps_good_items() {
        let scratch = Scratch::new("partial");
        let src = scratch.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("good"), b"ok").unwrap();
        let dest_dir = scratch.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        let sources = vec![src.join("missing"), src.join("good")];
        let outcome = transfer_batch_with(&sources, &dest_dir, true, Conflict::KeepBoth,
            &CancellationToken::new(), &mut |_| {});
        assert_eq!((outcome.completed, outcome.failed), (1, 1));
        assert_eq!(outcome.failed_sources, vec![src.join("missing")]);
        assert!(!dest_dir.join("missing").exists());
        assert_eq!(fs::read(dest_dir.join("good")).unwrap(), b"ok");
        assert!(!src.join("good").exists());
        assert_eq!(staging_left(&dest_dir), 0);
    }

    #[test]
    fn unreadable_file_fails_cleanly_and_keeps_source() {
        let scratch = Scratch::new("unreadable");
        let src = scratch.path().join("secret");
        fs::write(&src, b"data").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&src).is_ok() {
            // Privileged runs can read mode 000 files; the permission path is not exercisable.
            eprintln!("skipped: process can read mode 000 files");
            return;
        }
        let dest_dir = scratch.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        let outcome = transfer_batch_with(std::slice::from_ref(&src), &dest_dir, false,
            Conflict::KeepBoth, &CancellationToken::new(), &mut |_| {});
        assert_eq!(outcome.failed, 1);
        assert!(!dest_dir.join("secret").exists());
        assert_eq!(staging_left(&dest_dir), 0);
        fs::set_permissions(&src, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"data");
    }

    #[test]
    fn move_keeps_collision_and_undo_refuses_to_overwrite() {
        let scratch = Scratch::new("move");
        let src = scratch.path().join("src");
        let dst = scratch.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a"), b"moving").unwrap();
        fs::write(dst.join("a"), b"existing").unwrap();
        let cancel = CancellationToken::new();
        let outcome = transfer_batch_with(&[src.join("a")], &dst, true, Conflict::KeepBoth, &cancel, &mut |_| {});
        assert_eq!(outcome.completed, 1);
        assert!(!src.join("a").exists());
        assert_eq!(fs::read(dst.join("a")).unwrap(), b"existing");
        assert_eq!(fs::read(dst.join("a (2)")).unwrap(), b"moving");

        let entry = outcome.undo[0].clone();
        fs::write(src.join("a"), b"reoccupied").unwrap();
        assert!(undo(&entry).is_err(), "undo must not overwrite a reoccupied source");
        assert_eq!(fs::read(dst.join("a (2)")).unwrap(), b"moving");
        fs::remove_file(src.join("a")).unwrap();
        undo(&entry).unwrap();
        assert_eq!(fs::read(src.join("a")).unwrap(), b"moving");
        assert!(!dst.join("a (2)").exists());
    }

    #[test]
    fn created_folder_undo_only_removes_empty_directories() {
        let scratch = Scratch::new("newfolder");
        let folder = scratch.path().join("New Folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("keep"), b"x").unwrap();
        assert!(undo(&Undo::Created { path: folder.clone() }).is_err());
        assert!(folder.join("keep").exists());
        fs::remove_file(folder.join("keep")).unwrap();
        undo(&Undo::Created { path: folder.clone() }).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn trashed_item_undo_restores_original_path() {
        let scratch = Scratch::new("trash");
        let stored = scratch.path().join("files/item");
        let info = scratch.path().join("info/item.trashinfo");
        let original = scratch.path().join("item");
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::create_dir_all(info.parent().unwrap()).unwrap();
        fs::write(&stored, b"t").unwrap();
        fs::write(&info, b"[Trash Info]").unwrap();
        undo(&Undo::Trashed { original: original.clone(), stored: stored.clone(), info: info.clone() }).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"t");
        assert!(!stored.exists());
        assert!(!info.exists());
    }

    #[test]
    fn cross_device_move_verifies_then_removes_source() {
        let shm = Path::new("/dev/shm");
        let scratch = Scratch::new("xdev");
        if !shm.is_dir() || fs::metadata(shm).unwrap().dev() == fs::metadata(scratch.path()).unwrap().dev() {
            eprintln!("skipped: no second filesystem available at /dev/shm");
            return;
        }
        let src = shm.join(format!("flamewm-jobs-xdev-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src);
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/f"), b"cross").unwrap();
        let dest = scratch.path().join("moved");
        let result = move_publish(&src, &dest, false, &CancellationToken::new(), &mut |_| {});
        let _ = fs::remove_dir_all(&src);
        result.unwrap();
        assert_eq!(fs::read(dest.join("sub/f")).unwrap(), b"cross");
    }

    #[test]
    fn counting_reports_symlink_as_item_and_is_cancellable() {
        let scratch = Scratch::new("count");
        let dir = scratch.path().join("d");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f"), b"12345").unwrap();
        symlink("/", dir.join("root")).unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(count_tree(&dir, &cancel).unwrap(), Totals { items: 3, bytes: 5, errors: 0 });
        cancel.cancel();
        assert!(count_tree(&dir, &cancel).is_err());
    }
}
