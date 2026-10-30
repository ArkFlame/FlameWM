//! Native `FlameWM` assistant: one chat window over the local Hermes API server.
//!
//! The window owns presentation, paging and worker threads. Lifecycle, HTTP/SSE and the durable
//! history come from `flamewm-assistant-core`. Blocking work (probing, installing, starting,
//! streaming a turn) runs on short-lived threads that report back over a channel plus a wake pipe
//! that the window's event loop reads.

mod chat_header;
mod connect_ui;
mod instance;
mod model;
pub mod osd;
mod voice_setup_ui;
pub mod osd_host;

use osd::{Osd, OsdEvent, OsdInput, OsdState};
use osd_host::OsdProcess;

use std::cell::RefCell;
use std::env;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_assistant_core::{
    default_config_path, AssistantError, AssistantState, CancelToken, ChatStore, Detected, HermesClient, HermesConfig,
    LifeEvent, Message, MsgState, Paths, Recording, Role, SessionState, Speech, SpeechConfig,
    Stopper, TurnEvent, TurnOutcome, TurnRequest, Transcriber, candidate_urls,
    detect, discover, install_plan, read_env, transition,
    CommandSpeech, CommandTranscriber, apply_defaults, default_voice_dir, ready_model,
    SystemRunner, DEFAULT_BASE_URL,
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
    /// Whole seconds since the running turn started.
    Tick(u64),
    /// One line of installer output.
    Progress(String),
    /// Voice setup progress: percent and the step it is on.
    VoiceProgress(u8, String),
    /// The voice setup finished, with the TTS model directory or the reason it failed.
    VoiceSetup(Result<PathBuf, AssistantError>),
    /// A transcript, or the reason none was produced.
    Heard(Result<String, AssistantError>),
    /// Speech finished, was stopped, or failed.
    Spoke(Result<(), AssistantError>),
    /// The user clicked the speaking OSD.
    OsdClick,
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
    /// Newest sanitized installer output line, shown under "Installing Hermes...".
    install_line: String,
    /// The last prompt sent, restored into the input by Retry.
    last_prompt: String,
    tts_on: bool,
    /// Parsed `assistant-speech.conf`, or the reason it could not be read.
    speech_cfg: Result<SpeechConfig, String>,
    /// Open microphone capture while the user is recording.
    recording: Option<Recording>,
    /// True while the voice setup runs.
    downloading: bool,
    /// Text waiting for the TTS model to finish downloading.
    pending_speech: Option<String>,
    /// Voice OSD state, and the child process that draws it.
    osd: Osd,
    osd_process: OsdProcess,
    /// A voice transcript is waiting to be sent; the next submitted turn is a voice turn.
    voice_turn: bool,
    speaking: bool,
    speech_stop: Option<Stopper>,
    /// The header's clear-chat confirmation row is showing.
    clear_confirm: bool,
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
        let speech_cfg = match default_config_path() {
            Some(path) => SpeechConfig::load(&path).map_err(|error| error.to_string()),
            None => Ok(SpeechConfig::default()),
        };
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
            install_line: String::new(),
            last_prompt: String::new(),
            tts_on: false,
            speech_cfg,
            recording: None,
            downloading: false,
            pending_speech: None,
            osd: Osd::new(),
            osd_process: OsdProcess::default(),
            voice_turn: false,
            speaking: false,
            speech_stop: None,
            clear_confirm: false,
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
                self.install_line.clear();
                self.binary = Some(found.binary);
                self.state = transition(&self.state, LifeEvent::Detected(Some(found.version)));
                self.start();
            }
            Msg::Installed(Err(message)) => {
                self.installing = false;
                self.install_line.clear();
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
            Msg::Turn(event @ TurnEvent::Activity(_)) => connect_ui::on_turn_activity(self, event),
            Msg::Tick(secs) => connect_ui::working_tick(self, secs),
            Msg::Progress(line) => self.install_line = line,
            Msg::TurnDone(result) => self.finish_turn(result),
            Msg::VoiceProgress(percent, label) => self.notice = voice_setup_ui::progress_notice(percent, &label),
            Msg::VoiceSetup(result) => self.fetched(result),
            Msg::Heard(result) => self.heard(result),
            Msg::OsdClick => {
                if self.osd.input(OsdInput::Click) == Some(OsdEvent::StopSpeaking) {
                    self.stop_speech();
                }
            }
            Msg::Spoke(result) => {
                self.speaking = false;
                self.set_osd(OsdState::Hidden);
                self.speech_stop = None;
                if let Err(error) = result {
                    if error != AssistantError::Cancelled {
                        self.notice = error.to_string();
                    }
                }
            }
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
            let result = connect_ui::start_job(&paths, &binary, &url);
            bus.send(Msg::Started(result));
        });
    }

    /// Runs the user-confirmed installer on a worker thread. Output goes to the assistant log.
    fn install(&mut self) {
        self.confirm = false;
        self.installing = true;
        self.install_line.clear();
        self.notice.clear();
        let paths = self.paths.clone();
        let bus = self.bus.clone();
        thread::spawn(move || {
            let result = connect_ui::install_job(&paths, &bus);
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
        self.last_prompt = text.clone();
        self.cancel = CancelToken::new();
        if std::mem::take(&mut self.voice_turn) {
            self.set_osd(OsdState::Working);
        }
        self.history.set_pending(Some(String::new()));
        let config = hermes_config(&self.paths, &self.base_url());
        let bus = self.bus.clone();
        let cancel = self.cancel.clone();
        thread::spawn(move || turn_job(&bus, &config, session.as_deref(), &text, &cancel));
    }

    /// Deletes the saved chat and Hermes session after the user confirmed it.
    fn clear_chat(&mut self) {
        self.clear_confirm = false;
        if self.working {
            self.notice = "Stop the reply before clearing the chat.".into();
            return;
        }
        match self.store.clear() {
            Ok(()) => {
                self.history.clear();
                self.notice.clear();
                self.speech_stop = None;
            }
            Err(error) => self.notice = format!("Could not clear the chat: {error}"),
        }
    }

    fn cancel(&mut self) {
        if self.speaking || self.pending_speech.is_some() {
            self.stop_speech();
            return;
        }
        if self.osd.input(OsdInput::Escape) == Some(OsdEvent::CancelListening) {
            self.sync_osd();
        }
        if let Some(recording) = self.recording.take() {
            let _ = recording.finish();
            self.notice = "Recording discarded.".into();
            return;
        }
        if self.working {
            self.cancel.cancel();
        }
    }

    /// Mic press: start recording, or stop and transcribe. Downloads the STT model first if absent.
    fn toggle_mic(&mut self) {
        if !self.speech_in {
            self.notice = "Speech input is off. Enable it in Settings > Assistant.".into();
            return;
        }
        if let Some(recording) = self.recording.take() {
            let samples = recording.finish();
            self.set_osd(OsdState::Hidden);
            if samples.is_empty() {
                self.notice = "No audio was captured.".into();
            } else {
                self.transcribe(samples);
            }
            return;
        }
        if self.downloading {
            return;
        }
        let cfg = match self.speech_config() {
            Some(cfg) => cfg,
            None => return,
        };
        if self.installed_model().is_none() {
            self.setup_voice();
            return;
        }
        let argv = match flamewm_assistant_core::expand(&cfg.record, &[]) {
            Ok(argv) => argv,
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        match flamewm_assistant_core::start_recording(&argv) {
            Ok(recording) => {
                self.recording = Some(recording);
                self.notice = "Listening... press the mic again to stop.".into();
                self.set_osd(OsdState::Listening);
            }
            Err(error) => self.notice = error.to_string(),
        }
    }

    fn transcribe(&mut self, samples: Vec<i16>) {
        let Some(cfg) = self.speech_config() else { return };
        let Some(template) = cfg.transcribe.clone() else {
            self.notice = "Speech-to-text is not configured: set transcribe_command.".into();
            return;
        };
        let Some(model) = self.installed_model() else {
            self.notice = "Voice is not set up. Press the mic to set it up.".into();
            return;
        };
        self.notice = "Transcribing...".into();
        self.set_osd(OsdState::Working);
        let bus = self.bus.clone();
        thread::spawn(move || {
            let mut stt = CommandTranscriber::new(Box::new(SystemRunner), &template, model, CancelToken::new());
            bus.send(Msg::Heard(stt.transcribe(&samples)));
        });
    }

    fn heard(&mut self, result: Result<String, AssistantError>) {
        self.set_osd(OsdState::Hidden);
        match result {
            Ok(text) if text.trim().is_empty() => self.notice = "No speech was recognized.".into(),
            Ok(text) => {
                model::insert_text(&mut self.input, text.trim());
                self.notice.clear();
                self.voice_turn = true;
            }
            Err(error) => self.notice = error.to_string(),
        }
    }

    /// Speaks a reply. Downloads the TTS model first when it is missing.
    fn speak(&mut self, text: String) {
        let Some(cfg) = self.speech_config() else { return };
        let Some(synth) = cfg.synthesize.clone() else {
            self.notice = "Text-to-speech is not configured: set synthesize_command.".into();
            return;
        };
        let Some(model) = self.installed_model() else {
            self.pending_speech = Some(text);
            self.setup_voice();
            return;
        };
        self.stop_speech();
        let mut speech = CommandSpeech::new(
            Box::new(SystemRunner),
            &synth,
            &cfg.play,
            model,
            self.paths.state_dir.join("speech"),
        );
        self.speaking = true;
        self.set_osd(OsdState::Speaking);
        self.speech_stop = Some(speech.stopper());
        let bus = self.bus.clone();
        thread::spawn(move || bus.send(Msg::Spoke(speech.speak(&text))));
    }

    fn stop_speech(&mut self) {
        if let Some(stopper) = self.speech_stop.take() {
            stopper.stop();
        }
        self.speaking = false;
        self.pending_speech = None;
        self.set_osd(OsdState::Hidden);
    }

    /// Moves the OSD state machine and syncs its child process.
    fn set_osd(&mut self, state: OsdState) {
        self.osd.show(state);
        self.sync_osd();
    }

    fn sync_osd(&mut self) {
        let bus = self.bus.clone();
        self.osd_process.sync(self.osd.state(), move || bus.send(Msg::OsdClick));
    }

    fn fetched(&mut self, result: Result<PathBuf, AssistantError>) {
        self.downloading = false;
        match result {
            Ok(_) => {
                self.notice = "Voice ready. Press the mic to speak.".into();
                if let Some(text) = self.pending_speech.take() {
                    self.speak(text);
                }
            }
            Err(error) => {
                self.pending_speech = None;
                self.notice = error.to_string();
            }
        }
    }

    /// One click sets up the runtimes and models. Progress and the result arrive as messages.
    fn setup_voice(&mut self) {
        if self.downloading {
            return;
        }
        self.downloading = true;
        self.notice = "Setting up voice...".into();
        let progress_bus = self.bus.clone();
        let done_bus = self.bus.clone();
        voice_setup_ui::spawn(
            move |percent, label| progress_bus.send(Msg::VoiceProgress(percent, label)),
            move |result| done_bus.send(Msg::VoiceSetup(result)),
        );
    }

    /// The TTS model directory once voice setup has finished.
    fn installed_model(&self) -> Option<PathBuf> {
        ready_model(&default_voice_dir()?)
    }

    /// The parsed speech config, or a notice naming the problem.
    fn speech_config(&mut self) -> Option<SpeechConfig> {
        match &self.speech_cfg {
            Ok(cfg) => {
                let mut cfg = cfg.clone();
                if let Some(dir) = default_voice_dir() {
                    apply_defaults(&mut cfg, &dir);
                }
                Some(cfg)
            }
            Err(error) => {
                self.notice = format!("Speech config is invalid: {error}");
                None
            }
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
                    self.speak(text);
                }
            }
            Ok(TurnOutcome::Cancelled { partial: streamed }) => {
                let text = if streamed.is_empty() { partial } else { streamed };
                if !text.is_empty() {
                    self.save(Role::Assistant, &text, None, MsgState::Cancelled);
                }
                self.notice = "Stopped.".into();
            }
            Ok(TurnOutcome::Interrupted { partial: streamed, message }) => {
                let text = if streamed.is_empty() { partial } else { streamed };
                connect_ui::interrupted(self, &text, &message);
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
                if matches!(error, AssistantError::Unreachable(_) | AssistantError::Timeout) {
                    self.state = transition(&self.state, LifeEvent::GatewayLost);
                }
                connect_ui::failed(self, &error);
            }
        }
        if !self.speaking {
            self.set_osd(OsdState::Hidden);
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
            "assistant.clear" => self.clear_confirm = true,
            "assistant.clear-cancel" => self.clear_confirm = false,
            "assistant.clear-confirm" => self.clear_chat(),
            "assistant.stop" => self.cancel(),
            "assistant.tts" => {
                if self.speech_out {
                    self.tts_on = !self.tts_on;
                    if !self.tts_on {
                        self.stop_speech();
                    }
                } else {
                    self.notice = "Text-to-speech is off. Enable it in Settings > Assistant.".into();
                }
            }
            "assistant.speak-stop" => self.stop_speech(),
            "assistant.mic" | "assistant.record" => self.toggle_mic(),
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
            KeyCmd::Cancel if self.clear_confirm => self.clear_confirm = false,
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
                doc.text("loading-text", text.to_owned())?;
                doc.text("loading-detail", or_blank(self.install_line.clone()))
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
        let no_rows = rows.is_empty();
        doc.visible("list", !no_rows)?;
        doc.visible("empty", no_rows)?;
        doc.text("subtitle", chat_header::subtitle(&self.state, self.working).to_owned())?;
        let mic = self.speech_in && empty && !self.working;
        doc.visible("clear-confirm", self.clear_confirm)?;
        doc.visible("clear", !self.clear_confirm)?;
        doc.visible("working", self.working)?;
        doc.visible("speaking", self.speaking)?;
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

/// Offset that centres a window of `size` on the root span `root` (`axis` 0 = x, 1 = y).
fn centred(root: (u32, u32), size: u32, axis: usize) -> i32 {
    let span = if axis == 0 { root.0 } else { root.1 };
    (i64::from(span).saturating_sub(i64::from(size)) / 2).clamp(0, i64::from(i32::MAX)) as i32
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

/// One streamed turn. Every event and the final outcome go back to the window.
fn turn_job(bus: &Bus, config: &HermesConfig, session: Option<&str>, text: &str, cancel: &CancelToken) {
    let _ticker = connect_ui::Ticker::start(bus.clone());
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

    let root_size = flamewm_ui_x11::root_geometry().unwrap_or((1280, 800));
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
            x: centred(root_size, WIDTH, 0),
            y: centred(root_size, HEIGHT, 1),
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
        theme.values.insert("assistant.hermesUrl".into(), SettingValue::Text(" http://127.0.0.1:8642 ".into()));
        theme.values.insert("assistant.speechToText".into(), SettingValue::Boolean(true));
        assert_eq!(hermes_url(&theme), "http://127.0.0.1:8642");
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
