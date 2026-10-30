//! Native `FlameWM` assistant: one chat window over the local Hermes API server.
//!
//! The window owns presentation, paging and worker threads. Lifecycle, HTTP/SSE and the durable
//! history come from `flamewm-assistant-core`. Blocking work (probing, installing, starting,
//! streaming a turn) runs on short-lived threads that report back over a channel plus a wake pipe
//! that the window's event loop reads.

mod instance;
mod model;
pub mod osd;

use std::cell::RefCell;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_assistant_core::{
    AssistantError, AssistantState, CancelToken, ChatStore, Detected, HermesClient, HermesConfig,
    LifeEvent, Message, MsgState, Paths, Role, SessionState, Speech, TurnEvent, TurnOutcome,
    TurnRequest, UnconfiguredSpeech, candidate_urls, detect, discover, install_plan, read_env,
    start_gateway, transition, wait_until_ready, DEFAULT_BASE_URL,
};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiKeyEvent,
    UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document,
    run_with_controller_events_role_and_two_readable_fds,
};

use model::{ConnectStep, History, KeyCmd, Row, Screen, SLOTS};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-assistant.rwr"));
const WIDTH: u32 = 460;
const HEIGHT: u32 = 680;
/// How long a freshly started gateway may take to report healthy.
const CONNECT_DEADLINE: Duration = Duration::from_secs(60);
/// Wheel notches move the viewport by this many messages.
const WHEEL_STEP: isize = 3;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x0053_9bff;
const KEY_TINT: u32 = 0x00ff_ffff;

/// Messages from worker threads to the window.
enum Msg {
    /// Detection result and the URL of a healthy server, when one answered.
    Probed(Option<Detected>, Option<String>),
    Installed(Result<Detected, String>),
    Started(Result<String, String>),
    Turn(TurnEvent),
    TurnDone(Result<TurnOutcome, AssistantError>),
}

/// Sends a message to the window and pokes its wake pipe.
#[derive(Clone)]
struct Bus {
    tx: Sender<Msg>,
    wake: Arc<UnixStream>,
}

impl Bus {
    fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_ok() {
            // A full pipe already holds a wake byte, so a dropped write loses nothing.
            let _ = (&*self.wake).write(&[1]);
        }
    }
}

struct Assistant {
    paths: Paths,
    bus: Bus,
    theme: SettingsSnapshot,
    /// Hermes URL from settings. The ready URL, once connected, is in `state`.
    url: String,
    speech_in: bool,
    speech_out: bool,
    state: AssistantState,
    binary: Option<PathBuf>,
    probing: bool,
    installing: bool,
    confirm: bool,
    store: ChatStore,
    history: History,
    input: String,
    working: bool,
    cancel: CancelToken,
    notice: String,
    tts_on: bool,
}

impl model::Pager for ChatStore {
    fn page(&mut self, before: Option<u64>, limit: usize) -> Result<Vec<Message>, String> {
        self.load_page(before, limit).map_err(|error| error.to_string())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

fn turn_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{:x}-{:x}", now_ms(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: alpha,
    }
}

fn version_of(state: &AssistantState) -> String {
    match state {
        AssistantState::Stopped { version }
        | AssistantState::Starting { version, .. }
        | AssistantState::Ready { version, .. } => version.clone(),
        AssistantState::Error { version, .. } => version.clone().unwrap_or_default(),
        AssistantState::NotInstalled => String::new(),
    }
}

impl Assistant {
    fn new(
        paths: Paths,
        bus: Bus,
        theme: SettingsSnapshot,
        mut store: ChatStore,
    ) -> Result<Self, String> {
        let url = hermes_url(&theme);
        let speech_in = flag(&theme, "assistant.speechToText");
        let speech_out = flag(&theme, "assistant.textToSpeech");
        let total = store.len() as u64;
        let history = History::open(&mut store, total)?;
        Ok(Self {
            paths,
            bus,
            theme,
            url,
            speech_in,
            speech_out,
            state: AssistantState::NotInstalled,
            binary: None,
            probing: true,
            installing: false,
            confirm: false,
            store,
            history,
            input: String::new(),
            working: false,
            cancel: CancelToken::new(),
            notice: String::new(),
            tts_on: false,
        })
    }

    fn screen(&self) -> Screen {
        model::screen(&self.state, self.probing, self.installing, self.confirm)
    }

    /// Asks the worker for the current installation and server state.
    fn probe(&self) {
        let paths = self.paths.clone();
        let url = self.url.clone();
        let bus = self.bus.clone();
        thread::spawn(move || {
            let detected = detect(&paths.binary_candidates);
            let ready = detected.as_ref().and_then(|_| {
                let env = read_env(&paths.hermes_home);
                let candidates = candidate_urls(Some(&url), &env);
                discover(&candidates, env.api_key.as_deref()).map(|(found, _)| found)
            });
            bus.send(Msg::Probed(detected, ready));
        });
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Probed(detected, ready) => {
                self.probing = false;
                match detected {
                    None => {
                        self.state = transition(&self.state, LifeEvent::Detected(None));
                    }
                    Some(found) => {
                        self.binary = Some(found.binary);
                        let version = found.version;
                        self.state = transition(&self.state, LifeEvent::Detected(Some(version.clone())));
                        if let Some(url) = ready {
                            self.state = transition(&self.state, LifeEvent::Healthy { version, url });
                        }
                    }
                }
            }
            Msg::Installed(Ok(found)) => {
                self.installing = false;
                self.binary = Some(found.binary);
                self.state = transition(&self.state, LifeEvent::Detected(Some(found.version)));
                self.start();
            }
            Msg::Installed(Err(message)) => {
                self.installing = false;
                self.state = transition(&self.state, LifeEvent::Failed(message));
            }
            Msg::Started(Ok(url)) => {
                let version = version_of(&self.state);
                self.state = transition(&self.state, LifeEvent::Healthy { version, url });
            }
            Msg::Started(Err(message)) => {
                self.state = transition(&self.state, LifeEvent::Failed(message));
            }
            Msg::Turn(TurnEvent::Started { session }) => {
                if session.is_some() {
                    self.remember_session(session);
                }
            }
            Msg::Turn(TurnEvent::Delta(text)) => self.history.append_pending(&text),
            Msg::TurnDone(result) => self.finish_turn(result),
        }
    }

    /// The server URL in use: the ready URL when connected, else the configured one.
    fn base_url(&self) -> String {
        match &self.state {
            AssistantState::Ready { url, .. } => url.clone(),
            _ => self.url.clone(),
        }
    }

    fn connect(&mut self) {
        if self.probing || self.installing {
            return;
        }
        match model::connect_step(&self.state) {
            ConnectStep::Confirm => self.confirm = true,
            ConnectStep::Start => self.start(),
        }
    }

    /// Starts the gateway (or finds it already running) on a worker thread.
    fn start(&mut self) {
        self.notice.clear();
        self.state = transition(&self.state, LifeEvent::StartRequested);
        if !matches!(self.state, AssistantState::Starting { .. }) {
            return;
        }
        let Some(binary) = self.binary.clone() else {
            self.state = transition(&self.state, LifeEvent::Failed("Hermes is not installed".into()));
            return;
        };
        let paths = self.paths.clone();
        let url = self.url.clone();
        let bus = self.bus.clone();
        thread::spawn(move || {
            let result = start_job(&paths, &binary, &url);
            bus.send(Msg::Started(result));
        });
    }

    /// Runs the user-confirmed installer on a worker thread. Output goes to the assistant log.
    fn install(&mut self) {
        self.confirm = false;
        self.installing = true;
        self.notice.clear();
        let paths = self.paths.clone();
        let bus = self.bus.clone();
        thread::spawn(move || {
            let result = install_job(&paths);
            bus.send(Msg::Installed(result));
        });
    }

    fn submit(&mut self) {
        if self.working {
            return;
        }
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if !matches!(self.state, AssistantState::Ready { .. }) {
            self.notice = "Hermes is not connected.".into();
            return;
        }
        let turn = turn_id();
        let session = self.store.session().hermes_session.clone();
        if !self.save(Role::User, &text, Some(&turn), MsgState::Complete) {
            return;
        }
        self.input.clear();
        self.notice.clear();
        self.working = true;
        self.cancel = CancelToken::new();
        self.history.set_pending(Some(String::new()));
        let config = hermes_config(&self.paths, &self.base_url());
        let bus = self.bus.clone();
        let cancel = self.cancel.clone();
        thread::spawn(move || turn_job(&bus, &config, session.as_deref(), &text, &cancel));
    }

    fn cancel(&mut self) {
        if self.working {
            self.cancel.cancel();
        }
    }

    /// Persists a message and adds it to the viewport. Returns false when the store refused it.
    fn save(&mut self, role: Role, text: &str, turn: Option<&str>, state: MsgState) -> bool {
        let session = self.store.session().hermes_session.clone();
        match self.store.append(role, text, session.as_deref(), turn, state) {
            Ok(seq) => {
                self.history.push(Message {
                    seq,
                    ts_ms: now_ms(),
                    role,
                    text: text.to_owned(),
                    session,
                    turn: turn.map(str::to_owned),
                    state,
                });
                true
            }
            Err(error) => {
                self.notice = format!("Could not save the message: {error}");
                false
            }
        }
    }

    fn remember_session(&mut self, session: Option<String>) {
        let current = self.store.session().clone();
        if session.is_none() || current.hermes_session == session {
            return;
        }
        let state = SessionState { hermes_session: session, hermes_url: Some(self.base_url()) };
        if let Err(error) = self.store.set_session(state) {
            self.notice = format!("Could not save the session: {error}");
        }
    }

    fn finish_turn(&mut self, result: Result<TurnOutcome, AssistantError>) {
        self.working = false;
        let partial = self.history.take_pending().unwrap_or_default();
        match result {
            Ok(TurnOutcome::Completed { text, session }) => {
                self.remember_session(session);
                self.save(Role::Assistant, &text, None, MsgState::Complete);
                if self.tts_on {
                    let mut speech = UnconfiguredSpeech;
                    if let Err(error) = speech.speak(&text) {
                        self.notice = error.to_string();
                    }
                }
            }
            Ok(TurnOutcome::Cancelled { partial: streamed }) => {
                let text = if streamed.is_empty() { partial } else { streamed };
                if !text.is_empty() {
                    self.save(Role::Assistant, &text, None, MsgState::Cancelled);
                }
                self.notice = "Stopped.".into();
            }
            Err(error) if error.is_session_gone() => {
                let cleared = SessionState { hermes_session: None, hermes_url: None };
                if let Err(store) = self.store.set_session(cleared) {
                    self.notice = store.to_string();
                }
                self.save(
                    Role::SystemNote,
                    "The Hermes session expired. The next message starts a new one.",
                    None,
                    MsgState::Complete,
                );
            }
            Err(error) => {
                if !partial.is_empty() {
                    self.save(Role::Assistant, &partial, None, MsgState::Partial);
                }
                self.save(Role::SystemNote, &error.to_string(), None, MsgState::Error);
                if matches!(error, AssistantError::Unreachable(_) | AssistantError::Timeout) {
                    self.state = transition(&self.state, LifeEvent::GatewayLost);
                }
                self.notice = error.to_string();
            }
        }
    }

    fn wheel(&mut self, dy: f32) {
        if self.screen() != Screen::Chat || dy == 0.0 {
            return;
        }
        let delta = if dy < 0.0 { -WHEEL_STEP } else { WHEEL_STEP };
        if let Err(error) = self.history.scroll(&mut self.store, delta) {
            self.notice = error;
        }
    }

    fn action(&mut self, event: &UiActionEvent) {
        if event.action == "scroll" {
            if event.inside {
                let dy = event
                    .text
                    .as_deref()
                    .and_then(|text| text.split_once(','))
                    .and_then(|(_, dy)| dy.parse::<f32>().ok())
                    .unwrap_or(0.0);
                self.wheel(dy);
            }
            return;
        }
        if event.phase != UiActionPhase::Press {
            return;
        }
        match event.action.as_str() {
            "assistant.connect" => self.connect(),
            "assistant.install-confirm" => self.install(),
            "assistant.install-cancel" => self.confirm = false,
            "assistant.send" => self.submit(),
            "assistant.stop" => self.cancel(),
            "assistant.tts" => {
                if self.speech_out {
                    self.tts_on = !self.tts_on;
                }
            }
            "assistant.mic" | "assistant.record" => {
                if self.speech_in {
                    self.notice = "Speech input is not configured.".into();
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: &UiKeyEvent) {
        if key.phase != UiKeyPhase::Press {
            return;
        }
        let has = |name: &str| key.modifiers.iter().any(|m| m.eq_ignore_ascii_case(name));
        let shift = has("shift");
        let ctrl_or_alt = has("ctrl") || has("control") || has("alt");
        let command = model::key_command(&key.key, key.text.as_deref(), shift, ctrl_or_alt);
        if self.screen() != Screen::Chat {
            if command == KeyCmd::Cancel {
                self.confirm = false;
            }
            return;
        }
        match command {
            KeyCmd::Insert(text) => model::insert_text(&mut self.input, &text),
            KeyCmd::Newline => model::insert_text(&mut self.input, "\n"),
            KeyCmd::Backspace => {
                self.input.pop();
            }
            KeyCmd::Submit => self.submit(),
            KeyCmd::Cancel => self.cancel(),
            KeyCmd::Ignore => {}
        }
    }

    fn status_text(&self) -> String {
        if !self.notice.is_empty() {
            return self.notice.clone();
        }
        match &self.state {
            AssistantState::NotInstalled => "Hermes is not installed.".into(),
            AssistantState::Stopped { .. } => "Hermes is installed but not running.".into(),
            AssistantState::Error { message, .. } => message.clone(),
            AssistantState::Starting { .. } | AssistantState::Ready { .. } => String::new(),
        }
    }

    fn paint(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("assistant", rgba(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_border(doc, "assistant", &self.theme)?;
        doc.color_variable("--accent", rgba(accent(&self.theme), 255))?;
        let screen = self.screen();
        doc.visible("loading", screen == Screen::Loading)?;
        doc.visible("connect", screen == Screen::Connect)?;
        doc.visible("confirm", screen == Screen::Confirm)?;
        doc.visible("chat", screen == Screen::Chat)?;
        match screen {
            Screen::Loading => {
                let text = if self.installing { "Installing Hermes..." } else { "Loading..." };
                doc.text("loading-text", text.to_owned())
            }
            Screen::Connect => doc.text("connect-status", or_blank(self.status_text())),
            Screen::Confirm => doc.text("install-cmd", install_plan().shell_command),
            Screen::Chat => self.paint_chat(doc),
        }
    }

    fn paint_chat(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let rows: Vec<Row> = self.history.rows();
        for slot in 0..SLOTS {
            let row = rows.get(slot);
            let user = row.filter(|row| row.role == Role::User);
            let other = row.filter(|row| row.role != Role::User);
            doc.visible(&format!("u-{slot}"), user.is_some())?;
            doc.visible(&format!("a-{slot}"), other.is_some())?;
            if let Some(row) = user {
                doc.text(&format!("ut-{slot}"), row.text.clone())?;
            }
            if let Some(row) = other {
                doc.text(&format!("at-{slot}"), row.text.clone())?;
            }
        }
        let empty = self.input.is_empty();
        let mic = self.speech_in && empty && !self.working;
        doc.visible("working", self.working)?;
        doc.visible("placeholder", empty)?;
        doc.visible("input", !empty)?;
        doc.text("input", self.input.clone())?;
        doc.visible("record", self.speech_in && !self.working)?;
        doc.visible("mic", mic)?;
        doc.visible("send", !self.working && !mic)?;
        doc.visible("stop", self.working)?;
        doc.visible("tts", self.speech_out)?;
        let tts = if self.tts_on { rgba(accent(&self.theme), 255) } else { rgba(KEY_TINT, 36) };
        doc.background("tts", tts)?;
        doc.text("notice", or_blank(self.notice.clone()))
    }
}

fn or_blank(text: String) -> String {
    if text.is_empty() { "\u{200b}".to_owned() } else { text }
}

fn flag(theme: &SettingsSnapshot, key: &str) -> bool {
    matches!(theme.values.get(key), Some(SettingValue::Boolean(true)))
}

fn hermes_url(theme: &SettingsSnapshot) -> String {
    match theme.values.get("assistant.hermesUrl") {
        Some(SettingValue::Text(url)) if !url.trim().is_empty() => url.trim().to_owned(),
        _ => DEFAULT_BASE_URL.to_owned(),
    }
}

fn accent(theme: &SettingsSnapshot) -> u32 {
    match theme.values.get("accent") {
        Some(SettingValue::Rgb(rgb)) => rgb & 0x00ff_ffff,
        _ => ACCENT_RGB,
    }
}

fn hermes_config(paths: &Paths, url: &str) -> HermesConfig {
    HermesConfig {
        base_url: url.to_owned(),
        api_key: read_env(&paths.hermes_home).api_key,
        ..HermesConfig::default()
    }
}

/// Starts the gateway unless it already answers, then waits until it reports healthy.
fn start_job(paths: &Paths, binary: &Path, url: &str) -> Result<String, String> {
    let env = read_env(&paths.hermes_home);
    let candidates = candidate_urls(Some(url), &env);
    let key = env.api_key.as_deref();
    if let Some((found, _)) = discover(&candidates, key) {
        return Ok(found);
    }
    start_gateway(binary, paths, &env).map_err(|error| error.to_string())?;
    wait_until_ready(&candidates, key, CONNECT_DEADLINE, &CancelToken::new())
        .map(|(found, _)| found)
        .map_err(|error| error.to_string())
}

/// Runs the install script from `install_plan` in a shell with no terminal. Output goes to the log.
fn install_job(paths: &Paths) -> Result<Detected, String> {
    let plan = install_plan();
    fs::create_dir_all(&paths.state_dir).map_err(|error| error.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_path())
        .map_err(|error| error.to_string())?;
    let stderr = log.try_clone().map_err(|error| error.to_string())?;
    let status = Command::new("sh")
        .arg("-c")
        .arg(&plan.shell_command)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .status()
        .map_err(|error| format!("cannot run the installer: {error}"))?;
    if !status.success() {
        return Err(format!("the installer exited with {status}"));
    }
    detect(&paths.binary_candidates)
        .ok_or_else(|| "Hermes is still not found after the installer ran".to_owned())
}

/// One streamed turn. Every event and the final outcome go back to the window.
fn turn_job(bus: &Bus, config: &HermesConfig, session: Option<&str>, text: &str, cancel: &CancelToken) {
    let result = HermesClient::new(config).and_then(|client| {
        let request = TurnRequest::new(text, session);
        client.run_turn(&request, cancel, &mut |event| bus.send(Msg::Turn(event)))
    });
    bus.send(Msg::TurnDone(result));
}

/// Drains the wake pipe. It carries only "something changed" bytes, so the data is discarded.
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
    reader.set_nonblocking(true).map_err(|error| error.to_string())?;
    writer.set_nonblocking(true).map_err(|error| error.to_string())?;
    Ok((reader, writer))
}

/// Starts the window. Returns `Ok` without a window when this launch toggled an open one off.
pub fn run() -> Result<(), String> {
    let paths = Paths::for_user().ok_or("no home directory for the assistant")?;
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let pid_file = instance::pid_path(runtime_dir.as_deref(), Some(&paths.state_dir))
        .ok_or("no runtime or state folder for the assistant pid file")?;
    if instance::claim(&pid_file, Path::new("/proc")) == instance::Claim::Toggled {
        return Ok(());
    }
    let result = show(paths);
    instance::release(&pid_file);
    result
}

fn show(paths: Paths) -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let store = ChatStore::open(&paths.state_dir).map_err(|error| error.to_string())?;

    let (mut wake_reader, wake_writer) = nonblocking_pair()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let bus = Bus { tx, wake: Arc::new(wake_writer) };
    let app = Assistant::new(paths, bus, theme, store)?;
    app.paint(&mut document)?;
    app.probe();

    let fd = wake_reader.as_raw_fd();
    let app = Rc::new(RefCell::new(app));
    let event_app = Rc::clone(&app);
    let wake_app = Rc::clone(&app);
    run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 140,
            y: 80,
            title: "FlameWM Assistant".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-assistant".into(),
                class: "flamewm-assistant".into(),
            }),
        },
        UiWindowRole::Normal,
        fd,
        move || drain(&mut wake_reader),
        None,
        move |event, doc| {
            let mut app = event_app.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action),
                UiControllerEvent::Key(key) => app.key(key),
                UiControllerEvent::Drag(_) | UiControllerEvent::Resized { .. } => {}
            }
            app.paint(doc)
        },
        move |doc| {
            let mut app = wake_app.borrow_mut();
            let mut changed = false;
            while let Ok(msg) = rx.try_recv() {
                app.on_msg(msg);
                changed = true;
            }
            if changed {
                app.paint(doc)?;
            }
            Ok(())
        },
    )
}

/// Settings snapshot for theme and assistant keys; defaults apply when the control service is down.
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

fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let installed = env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr").then(|| usr.join("share/flamewm"))
    });
    installed
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or(source)
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
        packaged_root: packaged_root(),
        opacity_percent: opacity,
        blur_percent: blur,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_drive_url_and_speech_flags_with_safe_defaults() {
        let mut theme = SettingsSnapshot::new(1);
        assert_eq!(hermes_url(&theme), DEFAULT_BASE_URL);
        assert!(!flag(&theme, "assistant.speechToText"));
        theme.values.insert("assistant.hermesUrl".into(), SettingValue::Text(" http://127.0.0.1:8645 ".into()));
        theme.values.insert("assistant.speechToText".into(), SettingValue::Boolean(true));
        assert_eq!(hermes_url(&theme), "http://127.0.0.1:8645");
        assert!(flag(&theme, "assistant.speechToText"));
        assert!(!flag(&theme, "assistant.textToSpeech"));
    }

    #[test]
    fn accent_follows_the_settings_key() {
        let mut theme = SettingsSnapshot::new(1);
        assert_eq!(accent(&theme), ACCENT_RGB);
        theme.values.insert("accent".into(), SettingValue::Rgb(0x00ef_4048));
        assert_eq!(accent(&theme), 0x00ef_4048);
    }

    #[test]
    fn blank_text_keeps_a_line_box() {
        assert_eq!(or_blank(String::new()), "\u{200b}");
        assert_eq!(or_blank("x".into()), "x");
    }

    #[test]
    fn version_is_kept_across_starting_and_errors() {
        assert_eq!(version_of(&AssistantState::Starting { version: "1".into(), attempts: 0 }), "1");
        assert_eq!(version_of(&AssistantState::Error { message: "e".into(), version: None }), "");
        assert_eq!(version_of(&AssistantState::NotInstalled), "");
    }
}
