//! `flamewm-terminal [--working-directory DIR] [-e CMD [ARGS...]]`. Without `-e` the user's shell starts.
use std::cell::RefCell;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use flamewm_api::settings::SettingsSnapshot;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignal, ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_x11::{
    UiControllerEvent, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity,
    decode_document, run_with_controller_events_role_and_two_readable_fds,
};

use flamewm_terminal::app::App;
use flamewm_terminal::pty::Launch;
use flamewm_terminal::session::SessionEvent;
use flamewm_terminal::settings_cache;
use flamewm_terminal::store::{self, TerminalConfig, ThemeStore};
use flamewm_terminal::theme;
use flamewm_terminal::wake::{self, ALL_TABS_CLOSED};

/// Compiled UI per pool tier, smallest first (see `pool.rs`). A window keeps the tier its grid needs.
const UI: [&[u8]; 3] = [
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-terminal-0.rwr")),
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-terminal-1.rwr")),
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-terminal-2.rwr")),
];
const _: () = assert!(UI.len() == flamewm_terminal::pool::TIERS.len());
const DEFAULT_WINDOW: (u32, u32) = (880, 540);

pub fn main() {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("--flamefetch-shell") {
        let shell = arguments.next().unwrap_or_else(|| "/bin/sh".into());
        flamewm_terminal::fetch::exec_shell(&shell, &arguments.collect::<Vec<_>>());
    }
    if let Err(error) = run() {
        if error != ALL_TABS_CLOSED {
            eprintln!("flamewm-terminal: {error}");
            std::process::exit(1);
        }
    }
}

struct Options {
    working_directory: Option<PathBuf>,
    command: Option<Launch>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let mut options = Options {
        working_directory: None,
        command: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "FlameWM Terminal — usage: flamewm-terminal [--working-directory DIR] [-e CMD [ARGS...]]"
                );
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("flamewm-terminal {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--working-directory" => {
                let dir = args.next().ok_or("--working-directory needs a directory")?;
                options.working_directory = Some(PathBuf::from(dir));
            }
            "-e" | "--command" => {
                let program = args.next().ok_or("-e needs a command")?;
                options.command = Some(Launch {
                    program,
                    args: args.by_ref().collect(),
                    cwd: None,
                });
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(options))
}

fn run() -> Result<(), String> {
    let Some(options) = parse_args(std::env::args().skip(1))? else {
        return Ok(());
    };
    flamewm_profiler::init_process("flamewm-terminal");
    // One-shot: startup's font and config temporaries stay free-but-resident in the malloc arena.
    // A single sleeping thread hands them back; no periodic wake is added.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        flamewm_ui_x11::release_free_heap();
    });
    if flamewm_profiler::enabled() {
        // No reactor timer here: a plain thread drains the span window every interval, as the explorer does.
        let interval = Duration::from_secs(flamewm_profiler::profile_interval_secs());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(interval);
                let _ = flamewm_profiler::report_window();
            }
        });
    }
    let cwd = options
        .working_directory
        .map(|dir| fs::canonicalize(&dir).map_err(|error| format!("{}: {error}", dir.display())))
        .transpose()?;
    let config_dir = store::config_dir();
    let config_path = config_dir.join("terminal.toml");
    let config = fs::read_to_string(&config_path)
        .ok()
        .and_then(|text| TerminalConfig::from_toml(&text).ok())
        .unwrap_or_default();
    let store = ThemeStore::new(config_dir.join("themes"));
    let (events_tx, events) = mpsc::channel::<SessionEvent>();
    let (mut reader, writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    // First paint never waits on the desktop: it uses the last-known snapshot (or defaults on a first run). The live
    // `GetSettings` answer arrives on a worker thread and is applied from the wake socket.
    let cache_path = settings_cache::cache_path();
    let cached = settings_cache::load(&cache_path);
    let snapshot = cached.clone();
    let (settings_tx, settings_rx) = mpsc::channel::<SettingsSnapshot>();
    let settings_wake = writer.try_clone().map_err(|error| error.to_string())?;
    spawn_settings_fetch(cache_path, cached, settings_tx, settings_wake);
    let mut app = App::new(
        config,
        config_path,
        store,
        snapshot.clone(),
        flamewm_api::paths::workspace_packaged_root(),
        events_tx,
        writer,
        DEFAULT_WINDOW,
        None,
    );
    app.clipboard = flamewm_clipboard::Clipboard::connect().ok();
    let normal_shell = options.command.is_none();
    let launch = options.command.unwrap_or_else(|| Launch::shell(cwd));
    let launch = if normal_shell && flamewm_terminal::fetch::startup_enabled() {
        if let Ok(exe) = std::env::current_exe() {
            let mut args = vec!["--flamefetch-shell".to_owned(), launch.program];
            args.extend(launch.args);
            Launch {
                program: exe.to_string_lossy().into_owned(),
                args,
                cwd: launch.cwd,
            }
        } else {
            launch
        }
    } else {
        launch
    };
    app.open_tab(launch)?;
    let root = flamewm_api::paths::workspace_packaged_root();
    let backdrop = backdrop_for(snapshot.as_ref(), &root, app.config.background_opacity);
    let mut tier = app.pool_tier;
    let mut doc = decode_document(UI[tier])?;
    if let Some(spec) = backdrop {
        doc = doc.with_window_backdrop(spec);
    }
    app.project(&mut doc)?;
    let app = Rc::new(RefCell::new(app));
    let event_app = Rc::clone(&app);
    let wake_app = Rc::clone(&app);
    let fd = reader.as_raw_fd();
    let pending_snapshot = Rc::new(RefCell::new(None::<SettingsSnapshot>));
    let wake_snapshot = Rc::clone(&pending_snapshot);
    let notifications = ControlSignalClient::connect_session().ok().map(|signals| {
        let fd = signals.watch().fd;
        let queue = Rc::clone(&pending_snapshot);
        (
            fd,
            Box::new(move || {
                if signals
                    .on_ready(|signal| {
                        if let ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
                            *queue.borrow_mut() = Some(snapshot);
                        }
                    })
                    .is_ok()
                {
                    UiWakeDrain::Keep
                } else {
                    UiWakeDrain::Retire
                }
            }) as Box<dyn FnMut() -> UiWakeDrain>,
        )
    });
    run_with_controller_events_role_and_two_readable_fds(
        doc,
        UiWindowConfig {
            width: DEFAULT_WINDOW.0,
            height: DEFAULT_WINDOW.1,
            x: 80,
            y: 60,
            title: "FlameWM Terminal".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-terminal".to_owned(),
                class: "flamewm-terminal".to_owned(),
            }),
        },
        UiWindowRole::Normal,
        fd,
        move || {
            let mut bytes = [0_u8; 1024];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => return UiWakeDrain::Retire,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return UiWakeDrain::Keep;
                    }
                    Err(_) => return UiWakeDrain::Retire,
                }
            }
        },
        notifications,
        move |event, document| {
            let mut app = event_app.borrow_mut();
            let changed = match event {
                UiControllerEvent::Action(action) => app.pointer(action),
                UiControllerEvent::Key(key) => {
                    app.key(key);
                    true
                }
                UiControllerEvent::Resized { width, height } => {
                    app.set_window(*width, *height);
                    if app.pool_tier != tier {
                        // A replacement document starts with no projection: every node is written again.
                        tier = app.pool_tier;
                        document.replace_document(decode_document(UI[tier])?);
                        app.ui = Default::default();
                    }
                    true
                }
                UiControllerEvent::Drag(_) => true,
                UiControllerEvent::Visibility { .. } => false,
            };
            if changed {
                app.project(document)
            } else {
                Ok(())
            }
        },
        move |document| {
            let mut app = wake_app.borrow_mut();
            let mut settings_changed = false;
            if let Ok(snapshot) = settings_rx.try_recv() {
                settings_changed |= apply_settings(&mut app, snapshot);
            }
            if let Some(snapshot) = wake_snapshot.borrow_mut().take() {
                settings_changed |= apply_settings(&mut app, snapshot);
            }
            if settings_changed {
                // The window backdrop is fixed at creation; a changed snapshot re-requests it. An equal
                // spec is a no-op in the window loop, so this repaints only when the backdrop changes.
                document.request_window_backdrop(backdrop_for(
                    app.snapshot.as_ref(),
                    &root,
                    app.config.background_opacity,
                ));
            }
            let _wake = flamewm_profiler::start("terminal.wake");
            wake::drain_and_commit(&mut app, &events, settings_changed, |app| {
                let _project = flamewm_profiler::start("terminal.project");
                app.project(document)
            })
        },
    )
}

/// Startup fetches `GetSettings` off the main thread, in short attempts within a bounded budget.
const SETTINGS_ATTEMPT: Duration = Duration::from_millis(300);
const SETTINGS_BUDGET: Duration = Duration::from_millis(2_000);
const SETTINGS_RETRY_PAUSE: Duration = Duration::from_millis(100);

/// Fetches the live snapshot on a worker thread. A changed snapshot is persisted to the cache, sent to the UI loop and
/// announced on the wake socket; an unchanged one is only sent (the UI loop skips the repaint).
fn spawn_settings_fetch(
    cache_path: PathBuf,
    cached: Option<SettingsSnapshot>,
    tx: Sender<SettingsSnapshot>,
    mut wake: UnixStream,
) {
    std::thread::spawn(move || {
        let Some(snapshot) = initial_settings() else {
            return;
        };
        if cached.as_ref() != Some(&snapshot) {
            let _ = settings_cache::store(&cache_path, &snapshot);
        }
        if tx.send(snapshot).is_ok() {
            let _ = wake.write(&[1]);
        }
    });
}

/// The window backdrop for a settings snapshot: `None` when there is no snapshot or the window is opaque.
fn backdrop_for(
    snapshot: Option<&SettingsSnapshot>,
    packaged_root: &Path,
    opacity: u8,
) -> Option<flamewm_ui_x11::backdrop::WindowBackdrop> {
    snapshot.and_then(|s| theme::window_backdrop(s, packaged_root, opacity))
}

/// Whether `next` changes any value the view reads. A snapshot with the same values (a new revision only) is not a
/// change, so an unchanged live answer does not repaint the terminal.
fn settings_differ(current: Option<&SettingsSnapshot>, next: &SettingsSnapshot) -> bool {
    current.map(|current| &current.values) != Some(&next.values)
}

/// Installs a snapshot. Returns whether the palette inputs changed, so the caller repaints only then.
fn apply_settings(app: &mut App, snapshot: SettingsSnapshot) -> bool {
    let changed = settings_differ(app.snapshot.as_ref(), &snapshot);
    app.snapshot = Some(snapshot);
    if changed {
        app.reload_themes();
    }
    changed
}

fn initial_settings() -> Option<SettingsSnapshot> {
    let deadline = Instant::now() + SETTINGS_BUDGET;
    loop {
        // A fresh connection per attempt, so a late reply to an abandoned call cannot be read as the next answer.
        let client = ControlClient::connect(BusKind::Session).ok()?;
        match client.call_with_timeout(&ControlRequest::GetSettings, SETTINGS_ATTEMPT) {
            Ok(ControlResponse::Settings(snapshot)) => return Some(snapshot),
            _ if Instant::now() >= deadline => return None,
            _ => std::thread::sleep(SETTINGS_RETRY_PAUSE),
        }
    }
}

/// Packaged asset root for the default wallpaper: the installed share directory, else the source tree.
#[cfg(test)]
mod settings_apply_tests {
    use flamewm_api::settings::SettingValue;

    use super::*;

    #[test]
    fn same_values_under_a_new_revision_do_not_repaint() {
        let mut current = SettingsSnapshot::new(1);
        current.values.insert("accent".into(), SettingValue::Rgb(1));
        let mut next = current.clone();
        next.revision = 2;
        assert!(!settings_differ(Some(&current), &next));
    }

    #[test]
    fn an_equal_snapshot_requests_an_equal_backdrop_and_a_blur_change_a_new_one() {
        let root = Path::new("/share");
        let mut current = SettingsSnapshot::new(1);
        // Glass opt-in: a blur change only makes a new backdrop when the surface is translucent.
        current
            .values
            .insert("reduceTransparency".into(), SettingValue::Boolean(false));
        current
            .values
            .insert("wallpaper".into(), SettingValue::Text("/w.png".into()));
        current
            .values
            .insert("surfaceBlurPercent".into(), SettingValue::Percent(40));
        let mut same = current.clone();
        same.revision = 2;
        let mut blurrier = current.clone();
        blurrier
            .values
            .insert("surfaceBlurPercent".into(), SettingValue::Percent(60));
        let before = backdrop_for(Some(&current), root, 80);
        assert!(before.is_some());
        assert_eq!(before, backdrop_for(Some(&same), root, 80));
        assert_ne!(before, backdrop_for(Some(&blurrier), root, 80));
        assert_eq!(backdrop_for(Some(&current), root, 100), None);
        assert_eq!(backdrop_for(None, root, 80), None);
    }

    #[test]
    fn a_changed_value_or_a_first_arrival_repaints() {
        let mut current = SettingsSnapshot::new(1);
        current.values.insert("accent".into(), SettingValue::Rgb(1));
        let mut next = current.clone();
        next.values.insert("accent".into(), SettingValue::Rgb(2));
        assert!(settings_differ(Some(&current), &next));
        assert!(settings_differ(None, &next));
    }
}
