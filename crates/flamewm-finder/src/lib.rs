//! Native `FlameWM` file finder: one glass search window over a bounded filename index.
//!
//! The index is read from the on-disk cache at start, so the first query answers at once. A walk of
//! the home folder runs on one worker thread and replaces the index when it finishes. The window is
//! toggled by the session shortcut; see [`instance`].

mod format;
mod index;
mod instance;
mod keys;
mod kind;
mod palette;
mod quick;
mod preview;
mod recent_docs;
mod input;
mod paint;
mod search;
mod state;
mod verbs;
mod watch;

use std::cell::RefCell;
use std::env;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calloop::Interest;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_applications::ApplicationCatalog;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_reactor::{FdAction, Reactor};
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    RawPressWatch, UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess,
    UiKeyEvent, UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, WindowGeometrySlot,
    X11WindowIdentity, decode_document,
    run_with_controller_events_role_with_presses_and_reactor_geometry,
};

use index::{Entry, Index, Shared};
use keys::{Command, Modifiers};
use kind::{KINDS, Kind};
use palette::{CommandSpec, Mode};
use preview::Preview;
use search::{Hits, Query};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-finder.rwr"));
const WIDTH: u32 = 720;
const HEIGHT: u32 = 520;
/// Result rows that exist in the document; the list scrolls through them.
const ROW_SLOTS: usize = 8;
/// Preview lines that exist in the document.
const PREVIEW_SLOTS: usize = 24;
/// Name pieces per row that exist in the document; a name split by more terms merges the tail.
const SEGMENT_SLOTS: usize = 8;
/// Tiles per row in the grid view.
const GRID_COLUMNS: usize = 4;
/// Duplicate groups shown by the duplicates dialog.
const DUPLICATE_SLOTS: usize = 6;
const QUERY_LIMIT: usize = 256;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const PLACEHOLDER_INTERVAL: Duration = Duration::from_secs(5);
const PLACEHOLDERS: [&str; 4] = [
    "Search everything",
    "report type:pdf",
    "in:~/Projects config",
    "photo.jpg",
];
const TEXT_RGB: u32 = 0x00f5_f5f7;
const MUTED_RGB: u32 = 0x00a1_a1a6;
const SUBTLE_RGB: u32 = 0x0086_868b;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x003d_aee9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    None,
    Preview,
    Details,
    Help,
    Duplicates,
    Setup,
    /// Tab sheet of actions for the selected file or app.
    Verbs,
    /// Cancel-first sheet before a session-ending system action runs.
    Confirm,
}

/// One row or tile of the current view.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Entry(usize),
    Command(&'static CommandSpec),
    Scope(palette::Choice),
    /// Calculator answer; Enter puts the result into the search box.
    Calc(quick::Answer),
    /// Index into `Finder::apps`.
    App(usize),
    Action(&'static quick::SystemAction),
}

/// What the background worker sends the window: the application list first, then the file index.
enum Loaded {
    Apps(Vec<DesktopApplication>),
    /// Whether the session allows the log out action.
    Session(bool),
    /// The shared index holds the cached listing; the fresh walk still runs.
    Cached,
    /// The shared index holds the fresh walk.
    Files,
    /// The live watcher changed the shared index.
    Changed,
}

/// Text and accent colours for the current glass tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    text: u32,
    muted: u32,
    subtle: u32,
    accent: u32,
}

impl Palette {
    /// Light text on dark glass, dark text on light glass, and the Settings `accent` colour.
    fn for_theme(theme: &SettingsSnapshot) -> Self {
        let accent = match theme.values.get("accent") {
            Some(SettingValue::Text(hex)) => u32::from_str_radix(hex.trim_start_matches('#'), 16)
                .map_or(ACCENT_RGB, |rgb| rgb & 0x00ff_ffff),
            _ => ACCENT_RGB,
        };
        if flamewm_api::color::luminance(surface_fill(theme).rgb) > 150 {
            Self {
                text: 0x001d_1d1f,
                muted: 0x0051_5154,
                subtle: 0x006e_6e73,
                accent,
            }
        } else {
            Self {
                text: TEXT_RGB,
                muted: MUTED_RGB,
                subtle: SUBTLE_RGB,
                accent,
            }
        }
    }
}

/// Merges pieces past `max` into the last slot, so every name fits the document's segment nodes.
fn fold_pieces(mut pieces: Vec<(String, bool)>, max: usize) -> Vec<(String, bool)> {
    if pieces.len() <= max {
        return pieces;
    }
    let tail: String = pieces
        .split_off(max - 1)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    pieces.push((tail, false));
    pieces
}

struct Finder {
    home: String,
    index: Shared,
    walking: bool,
    query: String,
    parsed: Query,
    hits: Hits,
    selected: usize,
    first: usize,
    navigating: bool,
    placeholder: usize,
    overlay: Overlay,
    preview: Option<Preview>,
    /// The user's list or grid choice for search results. The home view is always tiles.
    grid: bool,
    groups: Vec<Vec<usize>>,
    theme: SettingsSnapshot,
    palette: Palette,
    pid_file: PathBuf,
    last_click: Option<(usize, Instant)>,
    apps: Vec<DesktopApplication>,
    /// Calculator answer, application matches and system actions for the current query.
    answer: Option<quick::Answer>,
    app_hits: Vec<usize>,
    actions: Vec<&'static quick::SystemAction>,
    /// Stays false until the session says logout is allowed (fail closed).
    logout: bool,
    /// System action waiting for the confirm sheet's Confirm row.
    pending: Option<&'static quick::SystemAction>,
    /// Actions for the selected result while the Tab sheet is open.
    verbs: &'static [verbs::Verb],
    /// Highlighted row of the Tab sheet.
    verb: usize,
    frecency: quick::Frecency,
    frecency_file: Option<PathBuf>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// Short mark for the type column: the extension in capitals, or `DIR` / `FILE`.
fn mark_for(entry: &Entry) -> String {
    if entry.is_dir {
        return "DIR".to_owned();
    }
    let name = entry.name();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 4 => {
            ext.to_uppercase()
        }
        _ => "FILE".to_owned(),
    }
}

fn meta_line(entry: &Entry, now: i64) -> String {
    let age = format::relative_time(entry.modified, now);
    let head = if entry.is_dir {
        "Folder".to_owned()
    } else {
        format::bytes(entry.size)
    };
    if age.is_empty() {
        head
    } else {
        format!("{head} · {age}")
    }
}


/// Initial settings snapshot for glass and theme; defaults apply when the control service is down.
fn initial_settings() -> SettingsSnapshot {
    ControlClient::connect(BusKind::Session)
        .ok()
        .and_then(|client| {
            client
                .call_with_timeout(&ControlRequest::GetSettings, Duration::from_millis(250))
                .ok()
        })
        .and_then(|reply| match reply {
            ControlResponse::Settings(snapshot) => Some(snapshot),
            _ => None,
        })
        .unwrap_or_else(|| SettingsSnapshot::new(0))
}

fn window_backdrop(snapshot: &SettingsSnapshot) -> flamewm_ui_x11::backdrop::WindowBackdrop {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    };
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 80,
    };
    let opacity = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 50,
    };
    let fit = text("wallpaperFit");
    flamewm_ui_x11::backdrop::WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: flamewm_api::paths::workspace_packaged_root(),
        opacity_percent: flamewm_ui_core::glass::window_backdrop_material(snapshot, opacity, blur)
            .0,
        blur_percent: flamewm_ui_core::glass::window_backdrop_material(snapshot, opacity, blur).1,
    }
}

/// Drains a wake pipe. The pipe carries only "something changed" bytes, so the data is discarded.
fn drain(reader: &mut UnixStream) -> UiWakeDrain {
    let mut buffer = [0_u8; 64];
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
}

fn nonblocking_pair() -> Result<(UnixStream, UnixStream), String> {
    let (reader, writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    writer
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    Ok((reader, writer))
}

/// Starts the window. Returns `Ok` without a window when this launch toggled an open finder off.
pub fn run() -> Result<(), String> {
    let home = env::var("HOME").unwrap_or_default();
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let cache_file = index::cache_path(env::var_os("XDG_CACHE_HOME"), env::var_os("HOME"));
    let cache_dir = cache_file.as_deref().and_then(Path::parent);
    let pid_file = instance::pid_path(runtime_dir.as_deref(), cache_dir)
        .ok_or_else(|| "no runtime or cache folder for the finder pid file".to_owned())?;
    if instance::claim(&pid_file, Path::new("/proc")) == instance::Claim::Toggled {
        return Ok(());
    }
    let result = show(&home, cache_file.as_deref(), &pid_file);
    instance::release(&pid_file);
    result
}

/// The next placeholder deadline when `now` has reached `due`; `None` while the tick is not due.
fn placeholder_due(now: Instant, due: Instant) -> Option<Instant> {
    (now >= due).then(|| now + PLACEHOLDER_INTERVAL)
}

fn show(home: &str, cache_file: Option<&Path>, pid_file: &Path) -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let shared = Shared::default();
    let mut app = Finder::new(home.to_owned(), shared.clone(), theme, pid_file.to_path_buf());
    app.paint(&mut document)?;

    app.frecency_file = cache_file
        .and_then(Path::parent)
        .map(|dir| dir.join("frecency.tsv"));
    app.frecency = app
        .frecency_file
        .as_deref()
        .map(quick::Frecency::load)
        .unwrap_or_default();

    let (mut index_reader, mut index_writer) = nonblocking_pair()?;
    let (sender, receiver) = mpsc::sync_channel::<Loaded>(1);
    let walk_cache = cache_file.map(Path::to_path_buf);
    let roots = index::default_roots(env::var_os("HOME"));
    let worker_index = shared;
    // Read once per launch: documents opened later show up after the next start.
    let docs = recent_docs::path(env::var_os("XDG_DATA_HOME"), env::var_os("HOME"))
        .map(|path| recent_docs::load(&path))
        .unwrap_or_default();
    std::thread::spawn(move || {
        // Applications first: the desktop catalog is a few hundred small files, so it lands
        // long before the file walk finishes.
        let apps = ApplicationCatalog::discover()
            .map(|catalog| catalog.all())
            .unwrap_or_default();
        if sender.send(Loaded::Apps(apps)).is_ok() {
            let _ = index_writer.write(&[1]);
        }
        if sender.send(Loaded::Session(quick::logout_allowed())).is_ok() {
            let _ = index_writer.write(&[1]);
        }
        // The cached index answers queries while the fresh walk runs; reading it here keeps the
        // window's first paint off the disk.
        if let Some(mut cached) = walk_cache.as_deref().and_then(index::load_cache) {
            cached.set_docs(docs.clone());
            *worker_index.write() = cached;
            if sender.send(Loaded::Cached).is_ok() {
                let _ = index_writer.write(&[1]);
            }
        }
        let mut fresh = index::walk(
            &roots,
            index::MAX_ENTRIES,
            index::MAX_DEPTH,
            index::WALK_BUDGET,
        );
        if let Some(path) = &walk_cache {
            let _ = index::save_cache(path, &fresh);
        }
        fresh.set_docs(docs);
        *worker_index.write() = fresh;
        if sender.send(Loaded::Files).is_ok() {
            let _ = index_writer.write(&[1]);
        }
        // The watcher keeps the shared index current for the life of the window.
        let result = watch::run(worker_index, roots, move || {
            if sender.try_send(Loaded::Changed).is_ok() {
                let _ = index_writer.write(&[1]);
            }
        });
        if let Err(error) = result {
            eprintln!("flamewm-finder: live index updates stopped: {error}");
        }
    });

    // One reactor owns both wake sources: the index-ready descriptor and the placeholder deadline.
    // The deadline is a reactor timer, so the placeholder adds no thread and no extra wake.
    let mut reactor = Reactor::new().map_err(|error| error.to_string())?;
    let placeholder_deadline = reactor.owned_wake_deadline();
    let mut next_placeholder = Instant::now() + PLACEHOLDER_INTERVAL;
    placeholder_deadline.set(Some(next_placeholder));
    let _index_registration = reactor
        .register_raw_fd_with_action(index_reader.as_raw_fd(), Interest::READ, move |_, _| {
            match drain(&mut index_reader) {
                UiWakeDrain::Keep => FdAction::Continue,
                UiWakeDrain::Retire => FdAction::Remove,
            }
        })
        .map_err(|error| error.to_string())?;

    let window = Rc::new(RefCell::new(app));
    let event_window = Rc::clone(&window);
    let wake_window = Rc::clone(&window);
    let incoming: Rc<RefCell<Receiver<Loaded>>> = Rc::new(RefCell::new(receiver));
    let wake_incoming = Rc::clone(&incoming);

    run_with_controller_events_role_with_presses_and_reactor_geometry(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 120,
            y: 90,
            title: "FlameWM Finder".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-finder".into(),
                class: "flamewm-finder".into(),
            }),
        },
        UiWindowRole::Normal,
        &mut reactor,
        WindowGeometrySlot::default(),
        RawPressWatch::default(),
        |_, _| Ok(()),
        move |event, doc| {
            let mut app = event_window.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action, doc),
                UiControllerEvent::Key(key) => app.key(key, doc),
                UiControllerEvent::Resized { .. }
                | UiControllerEvent::Drag(_)
                | UiControllerEvent::Visibility { .. } => Ok(()),
            }
        },
        move |doc| {
            let mut app = wake_window.borrow_mut();
            let mut changed = false;
            while let Ok(message) = wake_incoming.borrow().try_recv() {
                match message {
                    Loaded::Apps(apps) => app.accept_apps(apps),
                    Loaded::Session(logout) => app.accept_session(logout),
                    Loaded::Cached => app.accept_cached(),
                    Loaded::Files => app.accept_index(),
                    Loaded::Changed => app.accept_changes(),
                }
                changed = true;
            }
            if let Some(next) = placeholder_due(Instant::now(), next_placeholder) {
                next_placeholder = next;
                placeholder_deadline.set(Some(next));
                if app.query.is_empty() {
                    app.placeholder = (app.placeholder + 1) % PLACEHOLDERS.len();
                    changed = true;
                }
            }
            if changed {
                app.paint(doc)?;
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_tick_fires_once_per_interval_from_the_deadline() {
        let start = Instant::now();
        let due = start + PLACEHOLDER_INTERVAL;
        assert_eq!(super::placeholder_due(start, due), None);
        assert_eq!(
            super::placeholder_due(due, due),
            Some(due + PLACEHOLDER_INTERVAL)
        );
    }

    #[test]
    fn palette_accent_follows_the_settings_accent_key() {
        let mut theme = SettingsSnapshot::new(1);
        assert_eq!(Palette::for_theme(&theme).accent, ACCENT_RGB);
        theme.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#EF4048".to_owned()),
        );
        assert_eq!(Palette::for_theme(&theme).accent, 0x00ef_4048);
    }

    use super::*;

    #[test]
    fn marks_use_extension_or_type_word() {
        assert_eq!(
            mark_for(&Entry::new("/h/a/report.pdf".into(), false, 0, 0)),
            "PDF"
        );
        assert_eq!(
            mark_for(&Entry::new("/h/a/Makefile".into(), false, 0, 0)),
            "FILE"
        );
        assert_eq!(mark_for(&Entry::new("/h/a/src".into(), true, 0, 0)), "DIR");
        assert_eq!(
            mark_for(&Entry::new("/h/.bashrc".into(), false, 0, 0)),
            "FILE"
        );
    }

    #[test]
    fn meta_line_joins_size_and_age() {
        let file = Entry::new("/h/a.txt".into(), false, 1536, 1_000);
        assert_eq!(meta_line(&file, 1_000 + 120), "1.5 KB · 2m ago");
        let folder = Entry::new("/h/d".into(), true, 0, 0);
        assert_eq!(meta_line(&folder, 5), "Folder");
    }

    #[test]
    fn rgba_splits_channels() {
        assert_eq!(
            UiColor::from_rgb(0x0012_3456, 7),
            UiColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 7
            }
        );
    }
}
