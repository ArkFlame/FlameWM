//! The archive window: browse one archive, extract all or the selection, or create an archive from
//! paths. Each operation runs on one worker thread that reports through the library's `Reporter`
//! sink. The window only places the numbers it receives. Closing the window cancels the running
//! job; the library removes its `.partial` output on cancel, so nothing is left behind.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use flamewm_api::settings::SettingsSnapshot;
use flamewm_archiver::{CancelToken, Entry, Format, Kind, Progress, Summary, list};
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiKeyEvent,
    UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document,
    run_with_controller_events_role_and_two_readable_fds,
};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-archiver.rwr"));
const WIDTH: u32 = 560;
const HEIGHT: u32 = 520;
/// Rows of the entry list; must match the `row-N` elements in `ui/archiver/archiver.html`.
const ROWS: usize = 12;
/// Width of the progress track in logical px; must match `.track` in `ui/archiver/archiver.css`.
const TRACK_PX: f32 = 260.0;

/// What the window starts with.
pub enum Start {
    /// Browse `path`.
    Browse(PathBuf),
    /// Create `out` from `inputs` immediately, showing progress.
    Create { out: PathBuf, inputs: Vec<PathBuf> },
}

/// A unit of work: runs against the cancel token and reports progress through the sink.
type Work = Box<
    dyn FnOnce(&CancelToken, &mut dyn FnMut(Progress)) -> flamewm_archiver::Result<Summary> + Send,
>;

/// Messages from the worker thread to the window.
enum Reply {
    /// Result of the listing started by `App::open`.
    Listed {
        archive: PathBuf,
        result: Result<Vec<Entry>, String>,
    },
    Progress(Progress),
    Done {
        what: String,
        result: Result<Summary, String>,
        stopped: bool,
    },
}

struct Running {
    cancel: CancelToken,
    worker: JoinHandle<()>,
}

struct App {
    create: bool,
    archive: Option<PathBuf>,
    entries: Vec<Entry>,
    selected: BTreeSet<usize>,
    page: usize,
    status: String,
    progress: Option<Progress>,
    job: Option<Running>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    wake: UnixStream,
    /// Held by the running job so at most one wake byte is outstanding.
    wake_pending: Arc<AtomicBool>,
}

/// Opens the window and runs until it is closed or Escape is pressed.
pub fn run(start: Start) -> Result<(), String> {
    let mut document = decode_document(UI)?.with_keyboard_focus();
    let (mut reader, writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    writer
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let (tx, rx) = mpsc::channel();
    let mut app = App {
        create: false,
        archive: None,
        entries: Vec::new(),
        selected: BTreeSet::new(),
        page: 0,
        status: String::from("Ready"),
        progress: None,
        job: None,
        tx,
        rx,
        wake: writer,
        wake_pending: Arc::new(AtomicBool::new(false)),
    };
    match start {
        Start::Browse(path) => app.open(&path)?,
        Start::Create { out, inputs } => app.start_create(out, inputs)?,
    }
    app.paint(&mut document)?;
    let window = Rc::new(RefCell::new(app));
    let event_window = Rc::clone(&window);
    let wake_window = Rc::clone(&window);
    let fd = reader.as_raw_fd();
    let result = run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 75,
            y: 55,
            title: "FlameWM Archiver".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-archiver".into(),
                class: "flamewm-archiver".into(),
            }),
        },
        UiWindowRole::Normal,
        fd,
        move || {
            let mut buffer = [0_u8; 128];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return UiWakeDrain::Retire,
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return UiWakeDrain::Keep;
                    }
                    Err(_) => return UiWakeDrain::Retire,
                }
            }
        },
        None,
        move |event, doc| {
            let mut app = event_window.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action, doc),
                UiControllerEvent::Key(key) => app.key(key, doc),
                UiControllerEvent::Resized { .. } => app.paint(doc),
                UiControllerEvent::Drag(_) | UiControllerEvent::Visibility { .. } => Ok(()),
            }
        },
        move |doc| wake_window.borrow_mut().drain(doc),
    );
    // The window is gone: stop any running job so its partial output is removed before exit.
    window.borrow_mut().shutdown();
    result
}

impl App {
    /// Lists `path` on a worker thread: the window paints its first frame at once and the entries
    /// arrive as a `Reply::Listed`, so a large archive never holds up the window.
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.archive = Some(path.to_path_buf());
        self.status = String::from("Reading archive");
        let wake = self.wake.try_clone().map_err(|error| error.to_string())?;
        let tx = self.tx.clone();
        let archive = path.to_path_buf();
        std::thread::Builder::new()
            .name("flamewm-archiver-list".into())
            .spawn(move || {
                let mut wake = wake;
                let result = list(&archive).map_err(|error| error.to_string());
                if tx.send(Reply::Listed { archive, result }).is_ok() {
                    let _ = wake.write(&[1]);
                }
            })
            .map(drop)
            .map_err(|error| error.to_string())
    }

    fn start_create(&mut self, out: PathBuf, inputs: Vec<PathBuf>) -> Result<(), String> {
        self.create = true;
        self.status = format!("Creating {}", out.display());
        let what = format!("Created {}", out.display());
        self.start_job(
            what,
            Box::new(move |cancel, sink| flamewm_archiver::create(&out, &inputs, cancel, sink)),
        )
    }

    /// Starts `work` on a worker thread. Refused while another job runs.
    fn start_job(&mut self, what: String, work: Work) -> Result<(), String> {
        if self.job.is_some() {
            self.status = String::from("Busy: cancel the running job first");
            return Ok(());
        }
        let wake = self.wake.try_clone().map_err(|error| error.to_string())?;
        let tx = self.tx.clone();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        // One wake byte may be in flight at a time. Without the flag a fast job fills the socket
        // buffer, the write fails silently, and the final reply is never drained.
        let pending = Arc::new(AtomicBool::new(false));
        self.wake_pending = Arc::clone(&pending);
        let worker = std::thread::Builder::new()
            .name("flamewm-archiver-job".into())
            .spawn(move || {
                let mut signal = {
                    let mut wake = wake;
                    let pending = Arc::clone(&pending);
                    move || {
                        if !pending.swap(true, Ordering::Relaxed) {
                            let _ = wake.write(&[1]);
                        }
                    }
                };
                let result = {
                    let mut sink = |progress: Progress| {
                        if tx.send(Reply::Progress(progress)).is_ok() {
                            signal();
                        }
                    };
                    work(&worker_cancel, &mut sink)
                };
                let stopped = worker_cancel.is_cancelled();
                if tx
                    .send(Reply::Done {
                        what,
                        result: result.map_err(|error| error.to_string()),
                        stopped,
                    })
                    .is_ok()
                {
                    signal();
                }
            })
            .map_err(|error| error.to_string())?;
        self.job = Some(Running { cancel, worker });
        self.progress = None;
        self.status = String::from("Working");
        Ok(())
    }

    fn extract(&mut self, selection: Vec<PathBuf>) -> Result<(), String> {
        let Some(archive) = self.archive.clone() else {
            return Ok(());
        };
        let dest = free_destination(&archive);
        self.status = format!("Extracting to {}", dest.display());
        let what = format!("Extracted to {}", dest.display());
        self.start_job(
            what,
            Box::new(move |cancel, sink| {
                flamewm_archiver::extract(&archive, &dest, &selection, cancel, sink)
            }),
        )
    }

    fn action(
        &mut self,
        event: &UiActionEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.phase != UiActionPhase::Press {
            return Ok(());
        }
        match event.action.as_str() {
            "ar.prev" => self.turn(false),
            "ar.next" => self.turn(true),
            "ar.all" => self.extract(Vec::new())?,
            "ar.selected" => {
                if self.selected.is_empty() {
                    self.status = String::from("Select entries first");
                } else {
                    let paths = self
                        .selected
                        .iter()
                        .map(|&index| self.entries[index].path.clone())
                        .collect();
                    self.extract(paths)?;
                }
            }
            "ar.cancel" => self.cancel(),
            other => {
                if let Some(row) = other
                    .strip_prefix("ar.row")
                    .and_then(|number| number.parse::<usize>().ok())
                {
                    self.toggle(row);
                }
            }
        }
        self.paint(doc)
    }

    fn key(&mut self, key: &UiKeyEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press {
            return Ok(());
        }
        match key.key.as_str() {
            "escape" | "esc" => {
                self.shutdown();
                std::process::exit(0);
            }
            "up" | "pageup" => self.turn(false),
            "down" | "pagedown" => self.turn(true),
            _ => {}
        }
        self.paint(doc)
    }

    fn toggle(&mut self, row: usize) {
        let index = self.page * ROWS + row;
        if index < self.entries.len() && !self.selected.insert(index) {
            self.selected.remove(&index);
        }
    }

    fn turn(&mut self, forward: bool) {
        let pages = self.pages();
        if forward && self.page + 1 < pages {
            self.page += 1;
        } else if !forward && self.page > 0 {
            self.page -= 1;
        }
    }

    fn pages(&self) -> usize {
        self.entries.len().div_ceil(ROWS).max(1)
    }

    fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
            self.status = String::from("Cancelling");
        }
    }

    /// Folds one worker reply into the state. Painting is the caller's job, so a wake that
    /// drains thousands of progress messages still paints once.
    fn reply(&mut self, reply: Reply) {
        match reply {
            Reply::Listed { archive, result } => {
                // A listing for an archive that is no longer the window's target is stale.
                if self.archive.as_deref() != Some(archive.as_path()) {
                    return;
                }
                match result {
                    Ok(entries) => {
                        self.status = if entries.is_empty() {
                            String::from("Empty archive")
                        } else {
                            format!("{} entries", entries.len())
                        };
                        self.entries = entries;
                    }
                    Err(error) => self.status = format!("Cannot open: {error}"),
                }
            }
            Reply::Progress(progress) => self.progress = Some(progress),
            Reply::Done {
                what,
                result,
                stopped,
            } => {
                self.join_job();
                self.progress = None;
                self.status = match result {
                    Ok(summary) => format!(
                        "{what}: {} files, {} folders, {} skipped",
                        summary.files, summary.dirs, summary.skipped
                    ),
                    Err(_) if stopped => String::from("Cancelled, partial output removed"),
                    Err(error) => format!("Failed: {error}"),
                };
            }
        }
    }

    /// Applies every queued reply, then paints once. `on_wake` runs on each wake byte, so a job
    /// that reports per chunk would otherwise repaint for every chunk it produced and never
    /// present a frame until it finished.
    fn drain(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        // Re-arm the worker's wake: the byte that started this drain has been consumed.
        self.wake_pending.store(false, Ordering::Relaxed);
        let mut changed = false;
        while let Ok(reply) = self.rx.try_recv() {
            self.reply(reply);
            changed = true;
        }
        if changed {
            self.paint(doc)?;
        }
        Ok(())
    }

    fn join_job(&mut self) {
        if let Some(job) = self.job.take() {
            let _ = job.worker.join();
        }
    }

    /// Cancels the running job and waits for it, so its partial output is removed.
    fn shutdown(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.cancel();
            let _ = job.worker.join();
        }
    }

    fn paint(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&SettingsSnapshot::new(0));
        doc.background("archiver", UiColor::from_rgb(surface.rgb, surface.alpha))?;
        let name = self
            .archive
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(
                || {
                    String::from(if self.create {
                        "New archive"
                    } else {
                        "No archive"
                    })
                },
                |name| name.to_string_lossy().into_owned(),
            );
        doc.text("archive-name", name)?;
        doc.text("page", format!("Page {} / {}", self.page + 1, self.pages()))?;
        for row in 0..ROWS {
            let index = self.page * ROWS + row;
            let entry = self.entries.get(index);
            let row_id = format!("row-{row}");
            doc.visible(&row_id, entry.is_some())?;
            if let Some(entry) = entry {
                let mark = if self.selected.contains(&index) {
                    "x"
                } else {
                    " "
                };
                doc.text(
                    &format!("{row_id}-name"),
                    format!("[{mark}] {}", entry.path.display()),
                )?;
                doc.text(&format!("{row_id}-meta"), entry_meta(entry))?;
            }
        }
        doc.visible("list", !self.create)?;
        doc.visible("toolbar", !self.create)?;
        let busy = self.job.is_some();
        let (fraction, percent) = progress_fraction(self.progress);
        doc.visible("progress", busy)?;
        doc.size("progress-fill", TRACK_PX * fraction, 6.0)?;
        doc.text("progress-text", format!("{percent}%"))?;
        doc.text_ref("status", &self.status)
    }
}

/// Fraction in `0..=1` and whole percent of the running job. Bytes drive the bar when known,
/// otherwise entries.
fn progress_fraction(progress: Option<Progress>) -> (f32, u32) {
    let Some(progress) = progress else {
        return (0.0, 0);
    };
    let (done, total) = if progress.bytes_total > 0 {
        (progress.bytes_done, progress.bytes_total)
    } else {
        (progress.entries_done as u64, progress.entries_total as u64)
    };
    if total == 0 {
        return (0.0, 0);
    }
    let fraction = (done as f64 / total as f64).clamp(0.0, 1.0);
    (fraction as f32, (fraction * 100.0).round() as u32)
}

fn entry_meta(entry: &Entry) -> String {
    match entry.kind {
        Kind::File => human_size(entry.size),
        Kind::Dir => String::from("folder"),
        Kind::Symlink => String::from("link"),
        Kind::Other => String::from("other"),
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `dir/name.tar.gz` extracts to `dir/name`, or `dir/name-2` and so on when that exists.
/// The suffix list comes from the archive core so a new format never lands here twice.
fn free_destination(archive: &Path) -> PathBuf {
    let name = archive.file_name().map_or_else(
        || String::from("archive"),
        |name| name.to_string_lossy().into_owned(),
    );
    let stem = Format::strip_suffix(&name)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(name.as_str());
    let parent = archive.parent().unwrap_or(Path::new("."));
    let mut candidate = parent.join(stem);
    let mut number = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{stem}-{number}"));
        number += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_strips_archive_suffixes() {
        assert_eq!(
            free_destination(Path::new("/nonexistent-dir-xyz/photos.tar.gz")),
            PathBuf::from("/nonexistent-dir-xyz/photos")
        );
        assert_eq!(
            free_destination(Path::new("/nonexistent-dir-xyz/docs.zip")),
            PathBuf::from("/nonexistent-dir-xyz/docs")
        );
    }

    /// Every format the core accepts must lose its whole suffix, or `a.tar.xz` would extract into a
    /// directory named `a.tar`.
    #[test]
    fn destination_strips_every_supported_suffix() {
        for name in [
            "photos.tar",
            "photos.tar.gz",
            "photos.tgz",
            "photos.tar.xz",
            "photos.txz",
            "photos.tar.bz2",
            "photos.tbz2",
            "photos.tbz",
            "photos.tar.zst",
            "photos.tzst",
            "photos.7z",
            "photos.zip",
        ] {
            assert_eq!(
                free_destination(&Path::new("/nonexistent-dir-xyz").join(name)),
                PathBuf::from("/nonexistent-dir-xyz/photos"),
                "{name}"
            );
        }
    }

    #[test]
    fn progress_prefers_bytes_then_entries() {
        let bytes = Progress {
            bytes_done: 50,
            bytes_total: 200,
            entries_done: 9,
            entries_total: 10,
        };
        assert_eq!(progress_fraction(Some(bytes)), (0.25, 25));
        let entries = Progress {
            bytes_done: 0,
            bytes_total: 0,
            entries_done: 1,
            entries_total: 4,
        };
        assert_eq!(progress_fraction(Some(entries)), (0.25, 25));
        assert_eq!(progress_fraction(None), (0.0, 0));
    }

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KiB");
    }

    /// A window state with no archive open, plus the reply receiver the worker threads feed.
    fn idle_app() -> (App, Receiver<Reply>) {
        let (wake, _keep) = UnixStream::pair().expect("socket pair");
        let (tx, rx) = mpsc::channel();
        let app = App {
            create: false,
            archive: None,
            entries: Vec::new(),
            selected: BTreeSet::new(),
            page: 0,
            status: String::from("Ready"),
            progress: None,
            job: None,
            tx,
            rx: mpsc::channel().1,
            wake,
            wake_pending: Arc::new(AtomicBool::new(false)),
        };
        (app, rx)
    }

    #[test]
    fn listed_reply_applies_entries_for_the_open_archive() {
        let (mut app, _rx) = idle_app();
        app.archive = Some(PathBuf::from("/x/a.zip"));
        app.reply(Reply::Listed {
            archive: PathBuf::from("/x/a.zip"),
            result: Ok(vec![]),
        });
        assert_eq!(app.status, "Empty archive");
        app.reply(Reply::Listed {
            archive: PathBuf::from("/x/a.zip"),
            result: Err("bad".into()),
        });
        assert_eq!(app.status, "Cannot open: bad");
    }

    #[test]
    fn stale_listing_is_ignored() {
        let (mut app, _rx) = idle_app();
        app.archive = Some(PathBuf::from("/x/b.zip"));
        app.status = String::from("Reading archive");
        app.reply(Reply::Listed {
            archive: PathBuf::from("/x/a.zip"),
            result: Err("stale".into()),
        });
        assert_eq!(app.status, "Reading archive");
    }

    /// `open` returns before the archive is read: the listing arrives later as a `Reply::Listed`.
    #[test]
    fn open_lists_on_a_worker_and_replies() {
        let dir =
            std::env::temp_dir().join(format!("flamewm-archiver-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let input = dir.join("note.txt");
        std::fs::write(&input, b"hello").expect("input");
        let archive = dir.join("open-test.zip");
        flamewm_archiver::create(
            &archive,
            std::slice::from_ref(&input),
            &flamewm_archiver::CancelToken::new(),
            &mut |_| {},
        )
        .expect("create zip");

        let (mut app, _unused) = idle_app();
        let (tx, rx) = mpsc::channel();
        app.tx = tx;
        app.open(&archive).expect("open starts");
        assert_eq!(app.status, "Reading archive");
        let reply = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("listing reply");
        app.reply(reply);
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.status, "1 entries");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
