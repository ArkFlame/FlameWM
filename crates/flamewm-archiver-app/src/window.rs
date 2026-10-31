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
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use flamewm_api::settings::SettingsSnapshot;
use flamewm_archiver::{CancelToken, Entry, Kind, Progress, Summary, list};
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
}

/// Opens the window and runs until it is closed or Escape is pressed.
pub fn run(start: Start) -> Result<(), String> {
    let mut document = decode_document(UI)?;
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
    };
    match start {
        Start::Browse(path) => app.open(&path),
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
    fn open(&mut self, path: &Path) {
        self.archive = Some(path.to_path_buf());
        match list(path) {
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
        let worker = std::thread::Builder::new()
            .name("flamewm-archiver-job".into())
            .spawn(move || {
                let result = {
                    let mut sink = |progress: Progress| {
                        let _ = tx.send(Reply::Progress(progress));
                        let _ = (&wake).write(&[1]);
                    };
                    work(&worker_cancel, &mut sink)
                };
                let stopped = worker_cancel.is_cancelled();
                let _ = tx.send(Reply::Done {
                    what,
                    result: result.map_err(|error| error.to_string()),
                    stopped,
                });
                let _ = (&wake).write(&[1]);
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

    fn reply(&mut self, reply: Reply, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        match reply {
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
        self.paint(doc)
    }

    fn drain(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        while let Ok(reply) = self.rx.try_recv() {
            self.reply(reply, doc)?;
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
        doc.background("archiver", rgba(surface.rgb, surface.alpha))?;
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
        doc.text("status", self.status.clone())
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
fn free_destination(archive: &Path) -> PathBuf {
    let name = archive.file_name().map_or_else(
        || String::from("archive"),
        |name| name.to_string_lossy().into_owned(),
    );
    let stem = [".tar.gz", ".tgz", ".tar", ".zip"]
        .into_iter()
        .find_map(|ext| name.strip_suffix(ext))
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

fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: alpha,
    }
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
}
