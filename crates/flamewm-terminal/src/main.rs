//! `flamewm-terminal [--working-directory DIR] [-e CMD [ARGS...]]`. Without `-e` the user's shell starts.
use std::cell::RefCell;
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use flamewm_api::settings::SettingsSnapshot;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignal, ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_x11::{UiControllerEvent, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document, run_with_controller_events_role_and_two_readable_fds};

use flamewm_terminal::app::App;
use flamewm_terminal::pty::Launch;
use flamewm_terminal::session::SessionEvent;
use flamewm_terminal::store::{self, TerminalConfig, ThemeStore};
use flamewm_terminal::theme;

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-terminal.rwr"));
const DEFAULT_WINDOW: (u32, u32) = (880, 540);
/// Returned from the UI loop when the last tab closes. The run is a clean exit, not a failure.
const ALL_TABS_CLOSED: &str = "all tabs closed";

fn main() {
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
    let mut options = Options { working_directory: None, command: None };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("FlameWM Terminal — usage: flamewm-terminal [--working-directory DIR] [-e CMD [ARGS...]]");
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
                options.command = Some(Launch { program, args: args.by_ref().collect(), cwd: None });
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(options))
}

fn run() -> Result<(), String> {
    let Some(options) = parse_args(std::env::args().skip(1))? else { return Ok(()) };
    let cwd = match options.working_directory {
        Some(dir) => Some(fs::canonicalize(&dir).map_err(|error| format!("{}: {error}", dir.display()))?),
        None => None,
    };
    let config_dir = store::config_dir();
    let config_path = config_dir.join("terminal.toml");
    let config = fs::read_to_string(&config_path).ok().and_then(|text| TerminalConfig::from_toml(&text).ok()).unwrap_or_default();
    let store = ThemeStore::new(config_dir.join("themes"));
    let (events_tx, events) = mpsc::channel::<SessionEvent>();
    let (mut reader, writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader.set_nonblocking(true).map_err(|error| error.to_string())?;
    let snapshot = initial_settings();
    let mut app = App::new(
        config,
        config_path,
        store,
        snapshot.clone(),
        packaged_root(),
        events_tx,
        writer,
        DEFAULT_WINDOW,
        None,
    );
    app.clipboard = flamewm_clipboard::Clipboard::connect().ok();
    let launch = options.command.unwrap_or_else(|| Launch::shell(cwd));
    app.open_tab(launch)?;
    let backdrop = snapshot.as_ref().and_then(|s| theme::window_backdrop(s, &packaged_root()));
    let mut doc = decode_document(UI)?;
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
        (fd, Box::new(move || {
            if signals.on_ready(|signal| {
                if let ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
                    *queue.borrow_mut() = Some(snapshot);
                }
            }).is_ok() { UiWakeDrain::Keep } else { UiWakeDrain::Retire }
        }) as Box<dyn FnMut() -> UiWakeDrain>)
    });
    run_with_controller_events_role_and_two_readable_fds(
        doc,
        UiWindowConfig {
            width: DEFAULT_WINDOW.0,
            height: DEFAULT_WINDOW.1,
            x: 80,
            y: 60,
            title: "FlameWM Terminal".into(),
            identity: Some(X11WindowIdentity { instance: "flamewm-terminal".to_owned(), class: "flamewm-terminal".to_owned() }),
        },
        UiWindowRole::Normal,
        fd,
        move || {
            let mut bytes = [0_u8; 1024];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => return UiWakeDrain::Retire,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return UiWakeDrain::Keep,
                    Err(_) => return UiWakeDrain::Retire,
                }
            }
        },
        notifications,
        move |event, document| {
            let mut app = event_app.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.pointer(action),
                UiControllerEvent::Key(key) => app.key(key),
                UiControllerEvent::Resized { width, height } => app.set_window(*width, *height),
                _ => {}
            }
            app.project(document)
        },
        move |document| {
            let mut app = wake_app.borrow_mut();
            let mut changed = false;
            if let Some(snapshot) = wake_snapshot.borrow_mut().take() {
                app.snapshot = Some(snapshot);
                app.reload_themes();
                changed = true;
            }
            while let Ok(event) = events.try_recv() {
                changed = true;
                if app.session_event(event) {
                    return Err(ALL_TABS_CLOSED.to_owned());
                }
            }
            if changed { app.project(document) } else { Ok(()) }
        },
    )
}

fn initial_settings() -> Option<SettingsSnapshot> {
    let client = ControlClient::connect(BusKind::Session).ok()?;
    match client.call_with_timeout(&ControlRequest::GetSettings, Duration::from_millis(300)) {
        Ok(ControlResponse::Settings(snapshot)) => Some(snapshot),
        _ => None,
    }
}

/// Packaged asset root for the default wallpaper: the installed share directory, else the source tree.
fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let installed = std::env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr").then(|| usr.join("share/flamewm"))
    });
    installed.filter(|root| root.join("assets").is_dir()).unwrap_or(source)
}
