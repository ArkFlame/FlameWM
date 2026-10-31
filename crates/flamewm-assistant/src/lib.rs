//! Native `FlameWM` assistant: one chat window over ARK AI.
//!
//! The window owns presentation, paging, voice and worker threads. Chat requests go through the
//! ARK AI client in `flamewm-assistant-core`. Blocking work runs on short-lived threads that report
//! back over a channel plus a wake pipe that the window's event loop reads.

mod chat_header;
mod instance;
mod model;
mod popover;
pub mod osd;
pub mod osd_host;
pub mod voice_background;
/// Argument that runs the background voice setup instead of the window (see `voice_background`).
pub const VOICE_SETUP_ARG: &str = "--voice-setup";
mod voice_setup_ui;

use osd::{Osd, OsdEvent, OsdInput, OsdState};
use osd_host::OsdProcess;

use std::cell::RefCell;
use std::env;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_assistant_core::{
    apply_defaults, default_config_path, default_voice_dir, endpoint, expand, human_error, key_or_default, proxy_models,
    ready_model, resolve, start_recording, AssistantError, CancelToken, ChatStore, CommandSpeech, CommandTranscriber,
    Message, MsgState, OpenAiClient, Provider, Recording, Role, Selection, SessionState, Speech, SpeechConfig, Stopper,
    SystemRunner, Transcriber, Turn, FALLBACK_MODELS, HISTORY_TURNS,
};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    RawPress, RawPressWatch, UiActionEvent, UiActionPhase, UiColor, UiControllerEvent,
    UiDocumentAccess, UiKeyEvent, UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole,
    X11WindowIdentity, decode_document, run_with_controller_events_role_two_readable_and_presses,
};

use model::{History, KeyCmd, MenuRow, Row, SLOTS};

/// Rows in the model picker; longer lists page with prev and next.
const MODEL_ROWS: usize = 8;
const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-assistant.rwr"));
const WIDTH: u32 = 460;
const HEIGHT: u32 = 780;
/// Wheel notches move the viewport by this many messages.
const WHEEL_STEP: isize = 3;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x0053_9bff;
const KEY_TINT: u32 = 0x00ff_ffff;

/// Messages from worker threads to the window.
enum Msg {
    /// Model ids of the running Hermes proxy, or `None` when it is not running.
    Proxy(Option<Vec<String>>),
    /// A line for the notice, such as a fallback from the proxy to ARK AI.
    Notice(String),
    /// One step of the working animation.
    Tick,
    /// One streamed text piece of the running reply.
    Delta(String),
    /// The reply finished with its full text, or ended with the reason it did not.
    /// The label names the endpoint that answered, for error text.
    TurnDone(Result<String, AssistantError>, &'static str),
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
    /// `$XDG_STATE_HOME/flamewm/assistant`: chat history, saved settings and the speech scratch dir.
    state_dir: PathBuf,
    bus: Bus,
    theme: SettingsSnapshot,
    /// Key sent as the bearer token: the saved `assistant.apiKey`, or the public default.
    api_key: String,
    speech_in: bool,
    speech_out: bool,
    /// ARK AI model ids: the built-in list. Fetched never at open; the server list is not requested.
    models: Vec<String>,
    /// Model ids of the running Hermes proxy, learned from a turn; `None` until one reports them.
    proxy: Option<Vec<String>>,
    /// Provider and model the next turn uses.
    selection: Selection,
    model_menu: bool,
    model_page: usize,
    /// Typed filter for the model picker; the keyboard edits it while the menu is open.
    model_query: String,
    store: ChatStore,
    history: History,
    input: String,
    working: bool,
    /// Working-animation step; counts up while a turn runs.
    phase: u32,
    /// Cleared when the turn ends so its ticker thread exits.
    ticking: Arc<std::sync::atomic::AtomicBool>,
    cancel: CancelToken,
    notice: String,
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

fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: alpha,
    }
}

fn fallback_models() -> Vec<String> {
    FALLBACK_MODELS.iter().map(|id| (*id).to_owned()).collect()
}

impl Assistant {
    fn new(state_dir: PathBuf, bus: Bus, theme: SettingsSnapshot, mut store: ChatStore) -> Result<Self, String> {
        let api_key = api_key_setting(&theme);
        let speech_in = flag(&theme, "assistant.speechToText");
        let speech_out = flag(&theme, "assistant.textToSpeech");
        let total = store.len() as u64;
        let history = History::open(&mut store, total)?;
        let selection = {
            let session = store.session();
            Selection::from_state(session.provider.as_deref(), session.model.as_deref())
        };
        let speech_cfg = match default_config_path() {
            Some(path) => SpeechConfig::load(&path).map_err(|error| error.to_string()),
            None => Ok(SpeechConfig::default()),
        };
        Ok(Self {
            state_dir,
            bus,
            theme,
            api_key,
            speech_in,
            speech_out,
            models: fallback_models(),
            proxy: None,
            selection,
            model_menu: false,
            model_page: 0,
            model_query: String::new(),
            store,
            history,
            input: String::new(),
            working: false,
            phase: 0,
            ticking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel: CancelToken::new(),
            notice: String::new(),
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

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => self.phase = self.phase.wrapping_add(1),
            Msg::Proxy(models) => self.proxy = models,
            Msg::Notice(text) => self.notice = text,
            Msg::Delta(text) => self.history.append_pending(&text),
            Msg::TurnDone(result, label) => self.finish_turn(result, label),
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

    fn submit(&mut self) {
        if self.working {
            return;
        }
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if !self.save(Role::User, &text, MsgState::Complete) {
            return;
        }
        // The new message is already in the window's history, so it is the last turn sent.
        let turns = self.recent_turns();
        self.input.clear();
        self.notice.clear();
        self.working = true;
        self.phase = 0;
        self.cancel = CancelToken::new();
        self.start_ticker();
        if std::mem::take(&mut self.voice_turn) {
            self.set_osd(OsdState::Working);
        }
        self.history.set_pending(Some(String::new()));
        let selected = self.selection.clone();
        let key = self.api_key.clone();
        let bus = self.bus.clone();
        let cancel = self.cancel.clone();
        thread::spawn(move || turn_job(&bus, &selected, &key, &turns, &cancel));
    }

    /// The last messages on screen as chat turns, oldest first. The server keeps no session, so
    /// every request carries this window of the conversation.
    fn recent_turns(&self) -> Vec<Turn> {
        let rows = self.history.rows();
        let start = rows.len().saturating_sub(HISTORY_TURNS);
        rows[start..]
            .iter()
            .filter_map(|row| match row.role {
                Role::User => Some(Turn { role: "user", content: row.text.clone() }),
                Role::Assistant => Some(Turn { role: "assistant", content: row.text.clone() }),
                Role::SystemNote => None,
            })
            .collect()
    }

    /// Sends a `Tick` every 400 ms until the turn ends, so the working text animates.
    fn start_ticker(&self) {
        use std::sync::atomic::Ordering;
        self.ticking.store(true, Ordering::Relaxed);
        let (bus, ticking) = (self.bus.clone(), self.ticking.clone());
        thread::spawn(move || {
            while ticking.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(400));
                if ticking.load(Ordering::Relaxed) {
                    bus.send(Msg::Tick);
                }
            }
        });
    }

    fn finish_turn(&mut self, result: Result<String, AssistantError>, label: &str) {
        self.working = false;
        self.ticking.store(false, std::sync::atomic::Ordering::Relaxed);
        let partial = self.history.take_pending().unwrap_or_default();
        match result {
            Ok(text) => {
                self.save(Role::Assistant, &text, MsgState::Complete);
                if self.tts_on {
                    self.speak(text);
                }
            }
            Err(AssistantError::Cancelled) => {
                if !partial.is_empty() {
                    self.save(Role::Assistant, &partial, MsgState::Cancelled);
                }
                self.notice = "Stopped.".into();
            }
            Err(error) => {
                if !partial.is_empty() {
                    self.save(Role::Assistant, &partial, MsgState::Partial);
                }
                self.notice = human_error(&error, label);
            }
        }
        if !self.speaking {
            self.set_osd(OsdState::Hidden);
        }
    }

    /// Rows of the model picker: ARK AI first, then the proxy when it is running. Each group
    /// has a header row, and the filter applies to every group.
    fn menu_rows(&self) -> Vec<MenuRow> {
        let mut groups: Vec<(Provider, &[String])> = vec![(Provider::Ark, self.models.as_slice())];
        if let Some(ids) = &self.proxy {
            groups.push((Provider::HermesProxy, ids.as_slice()));
        }
        model::menu_rows(&groups, &self.model_query)
    }

    /// Adds `text` to the picker filter (or removes the last character) and starts at page 1.
    fn edit_model_query(&mut self, text: Option<&str>) {
        match text {
            Some(text) => self.model_query.push_str(text),
            None => {
                self.model_query.pop();
            }
        }
        self.model_page = 0;
    }

    fn toggle_model_menu(&mut self) {
        self.model_menu = !self.model_menu;
        self.model_query.clear();
        let current = self.selection.clone();
        let position = self.menu_rows().iter().position(|row| {
            matches!(row, MenuRow::Model { provider, id } if *provider == current.provider && *id == current.model)
        });
        self.model_page = position.map_or(0, |p| p / MODEL_ROWS);
    }

    /// Makes the model on row `slot` of the current page the one used from the next turn on, and
    /// saves it. Header rows do nothing.
    fn select_row(&mut self, slot: usize) {
        let rows = self.menu_rows();
        let Some(MenuRow::Model { provider, id }) = rows.get(self.model_page * MODEL_ROWS + slot) else { return };
        self.selection = Selection { provider: *provider, model: id.clone() };
        self.model_menu = false;
        let state = self.session_state();
        if let Err(error) = self.store.set_session(state) {
            self.notice = format!("Could not save the model: {error}");
        }
    }

    /// Slot of the first model row on the current page, for the Enter key.
    fn first_model_slot(&self) -> Option<usize> {
        let rows = self.menu_rows();
        let start = self.model_page * MODEL_ROWS;
        (0..MODEL_ROWS).find(|slot| matches!(rows.get(start + slot), Some(MenuRow::Model { .. })))
    }

    fn session_state(&self) -> SessionState {
        SessionState {
            provider: Some(self.selection.provider.id().to_owned()),
            model: Some(self.selection.model.clone()),
        }
    }

    /// Deletes the saved chat after the user confirmed it. The model choice is kept.
    fn clear_chat(&mut self) {
        self.clear_confirm = false;
        if self.working {
            self.notice = "Stop the reply before clearing the chat.".into();
            return;
        }
        let state = self.session_state();
        let cleared = self.store.clear().and_then(|()| self.store.set_session(state));
        match cleared {
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
        let Some(cfg) = self.speech_config() else { return };
        if self.installed_model().is_none() {
            self.setup_voice();
            return;
        }
        let argv = match expand(&cfg.record, &[]) {
            Ok(argv) => argv,
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        match start_recording(&argv) {
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
            self.state_dir.join("speech"),
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
    fn save(&mut self, role: Role, text: &str, state: MsgState) -> bool {
        match self.store.append(role, text, None, None, state) {
            Ok(seq) => {
                self.history.push(Message {
                    seq,
                    ts_ms: now_ms(),
                    role,
                    text: text.to_owned(),
                    session: None,
                    turn: None,
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

    fn wheel(&mut self, dy: f32) {
        if dy == 0.0 {
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
            "assistant.powered" => {
                let _ = std::process::Command::new("xdg-open").arg("https://ai.arkflame.com").spawn();
            }
            "assistant.send" => self.submit(),
            "assistant.model-toggle" => self.toggle_model_menu(),
            "assistant.model-prev" => self.model_page = self.model_page.saturating_sub(1),
            "assistant.model-next" => {
                let last = self.menu_rows().len().saturating_sub(1) / MODEL_ROWS;
                self.model_page = (self.model_page + 1).min(last);
            }
            action if action.starts_with("assistant.model.") => {
                if let Ok(slot) = action["assistant.model.".len()..].parse::<usize>() {
                    self.select_row(slot);
                }
            }
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
        if self.model_menu {
            // The picker owns the keyboard: typing filters it, Escape clears or closes it.
            match command {
                KeyCmd::Insert(text) => self.edit_model_query(Some(&text)),
                KeyCmd::Backspace => self.edit_model_query(None),
                KeyCmd::Cancel if !self.model_query.is_empty() => {
                    self.model_query.clear();
                    self.model_page = 0;
                }
                KeyCmd::Cancel => self.model_menu = false,
                KeyCmd::Submit => {
                    if let Some(slot) = self.first_model_slot() {
                        self.select_row(slot);
                    }
                }
                KeyCmd::Newline | KeyCmd::Ignore => {}
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

    fn paint(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("assistant", rgba(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_border(doc, "assistant", &self.theme)?;
        doc.color_variable("--accent", rgba(accent(&self.theme), 255))?;
        self.paint_chat(doc)
    }

    fn paint_models(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        doc.text("model", model_label(&pretty_model(&self.selection.model)))?;
        let open = self.model_menu;
        doc.visible("model-menu", open)?;
        let rows = self.menu_rows();
        doc.visible("model-empty", open && rows.is_empty())?;
        doc.visible("model-hint", open && self.model_query.is_empty())?;
        doc.text("model-query", self.model_query.clone())?;
        let pages = rows.len().div_ceil(MODEL_ROWS).max(1);
        doc.text("model-page", format!("{} / {}", self.model_page + 1, pages))?;
        for slot in 0..MODEL_ROWS {
            let row = rows.get(self.model_page * MODEL_ROWS + slot);
            let header = match row {
                Some(MenuRow::Header(provider)) => Some(*provider),
                _ => None,
            };
            doc.visible(&format!("mhead-{slot}"), open && header.is_some())?;
            if let Some(provider) = header {
                doc.text(&format!("mhead-text-{slot}"), provider.title().to_owned())?;
            }
            let model_row = match row {
                Some(MenuRow::Model { provider, id }) => Some((*provider, id)),
                _ => None,
            };
            doc.visible(&format!("mrow-{slot}"), open && model_row.is_some())?;
            if let Some((provider, id)) = model_row {
                let current = provider == self.selection.provider && *id == self.selection.model;
                doc.text(&format!("mtext-{slot}"), pretty_model(id))?;
                doc.visible(&format!("mcheck-{slot}"), current)?;
            }
        }
        Ok(())
    }

    fn paint_chat(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let rows: Vec<Row> = self.history.rows();
        for slot in 0..SLOTS {
            let row = rows.get(slot);
            let user = row.filter(|row| row.role == Role::User);
            let other = row.filter(|row| row.role != Role::User);
            // A wrapping message gets the fixed-width bubble so painted wrap matches measured height.
            let wide = user.is_some_and(|row| model::wraps(&row.text));
            doc.visible(&format!("u-{slot}"), user.is_some() && !wide)?;
            doc.visible(&format!("uw-{slot}"), wide)?;
            doc.visible(&format!("a-{slot}"), other.is_some())?;
            if let Some(row) = user {
                doc.text(&format!("ut-{slot}"), row.text.clone())?;
                doc.text(&format!("utw-{slot}"), row.text.clone())?;
            }
            if let Some(row) = other {
                doc.text(&format!("at-{slot}"), row.text.clone())?;
            }
        }
        let empty = self.input.is_empty();
        let no_rows = rows.is_empty();
        doc.visible("list", !no_rows && !self.model_menu)?;
        doc.visible("menu-fill", self.model_menu)?;
        doc.visible("empty", no_rows)?;
        let working = self.working.then_some(self.phase);
        doc.text("subtitle", chat_header::subtitle(working))?;
        doc.text("working-text", chat_header::working_text(self.phase))?;
        let ark = self.selection.provider == Provider::Ark;
        doc.visible("powered-ark", ark)?;
        doc.visible("powered-hermes", !ark)?;
        let mic = self.speech_in && empty && !self.working;
        self.paint_models(doc)?;
        doc.visible("clear-confirm", self.clear_confirm)?;
        doc.visible("clear", !self.clear_confirm)?;
        doc.visible("working", self.working)?;
        doc.visible("speaking", self.speaking)?;
        doc.visible("placeholder", empty)?;
        doc.visible("input", !empty)?;
        doc.visible("caret-empty", empty)?;
        doc.visible("caret", !empty)?;
        doc.text("input", model::keep_spaces(&self.input))?;
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

/// The `assistant.apiKey` setting, or the public default when it is missing or blank. A stored
/// value that is not a valid key is kept, so the request reports why it fails.
fn api_key_setting(theme: &SettingsSnapshot) -> String {
    match theme.values.get("assistant.apiKey") {
        Some(SettingValue::Text(key)) => key_or_default(Some(key)),
        _ => key_or_default(None),
    }
}

/// Display name for a model id: `solar-pro` -> `Solar Pro`.
fn pretty_model(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Short header label for a model id: the part after the last `/`, cut to 18 characters.
fn model_label(id: &str) -> String {
    let name = id.rsplit('/').next().unwrap_or(id);
    if name.chars().count() > 18 {
        format!("{}...", name.chars().take(15).collect::<String>())
    } else {
        name.to_owned()
    }
}

fn accent(theme: &SettingsSnapshot) -> u32 {
    match theme.values.get("accent") {
        Some(SettingValue::Rgb(rgb)) => rgb & 0x00ff_ffff,
        Some(SettingValue::Text(hex)) => {
            u32::from_str_radix(hex.trim_start_matches('#'), 16).map_or(ACCENT_RGB, |rgb| rgb & 0x00ff_ffff)
        }
        _ => ACCENT_RGB,
    }
}

/// Runs the proxy probe on its own thread, so a stopped turn does not wait for it. Returns `None`
/// when the probe fails or the turn is cancelled first.
fn probe_proxy_off_thread(cancel: &CancelToken) -> Option<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    let probe_cancel = cancel.clone();
    thread::spawn(move || {
        let _ = tx.send(proxy_models(&probe_cancel));
    });
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(found) => return found,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// One streamed reply. A selected Hermes proxy is probed first (300 ms limit, cancellable); when it
/// is gone the turn falls back to ARK AI with a notice. Each text piece and the final result go back
/// to the window.
fn turn_job(bus: &Bus, selected: &Selection, ark_key: &str, turns: &[Turn], cancel: &CancelToken) {
    let proxy_up = if selected.provider == Provider::HermesProxy {
        let found = probe_proxy_off_thread(cancel);
        let up = found.is_some();
        bus.send(Msg::Proxy(found));
        up
    } else {
        true
    };
    let (used, notice) = resolve(selected, proxy_up);
    if let Some(notice) = notice {
        bus.send(Msg::Notice(notice));
    }
    let ep = endpoint(used.provider, ark_key);
    let label = ep.label;
    let result = OpenAiClient::new(ep).and_then(|client| {
        client.chat(&used.model, turns, cancel, &mut |delta| bus.send(Msg::Delta(delta.to_owned())))
    });
    bus.send(Msg::TurnDone(result, label));
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

/// `$XDG_STATE_HOME/flamewm/assistant`, falling back to `~/.local/state`.
fn state_dir() -> Option<PathBuf> {
    let home = env::home_dir()?;
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"));
    Some(base.join("flamewm").join("assistant"))
}

/// Starts the window. Returns `Ok` without a window when this launch toggled an open one off.
pub fn run() -> Result<(), String> {
    let state_dir = state_dir().ok_or("no home directory for the assistant")?;
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let pid_file = instance::pid_path(runtime_dir.as_deref(), Some(&state_dir))
        .ok_or("no runtime or state folder for the assistant pid file")?;
    if instance::claim(&pid_file, Path::new("/proc")) == instance::Claim::Toggled {
        return Ok(());
    }
    let result = show(state_dir, None);
    instance::release(&pid_file);
    result
}

/// Parses `X Y WIDTH HEIGHT` after `--popover`.
#[must_use]
pub fn popover_anchor(args: &[String]) -> Option<popover::Anchor> {
    popover::parse_args(args)
}

/// Taskbar popover: the same toggle as `run`, but the window is an undecorated popup above `anchor`.
pub fn run_popover(anchor: popover::Anchor) -> Result<(), String> {
    let state_dir = state_dir().ok_or("no home directory for the assistant")?;
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let pid_file = instance::pid_path(runtime_dir.as_deref(), Some(&state_dir))
        .ok_or("no runtime or state folder for the assistant pid file")?;
    if instance::claim(&pid_file, Path::new("/proc")) == instance::Claim::Toggled {
        return Ok(());
    }
    let result = show(state_dir, Some(anchor));
    instance::release(&pid_file);
    match result {
        Err(error) if error == popover::DISMISSED => Ok(()),
        other => other,
    }
}

fn show(state_dir: PathBuf, anchor: Option<popover::Anchor>) -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let store = ChatStore::open(&state_dir).map_err(|error| error.to_string())?;

    let root_size = flamewm_ui_x11::root_geometry().unwrap_or((1280, 800));
    let window_height = anchor.map_or(HEIGHT, |anchor| HEIGHT.min(popover::max_height(root_size, anchor) as u32));
    model::set_window_height(window_height as usize);
    let (mut wake_reader, wake_writer) = nonblocking_pair()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let bus = Bus { tx, wake: Arc::new(wake_writer) };
    let mut app = Assistant::new(state_dir, bus, theme, store)?;
    app.paint(&mut document)?;

    let fd = wake_reader.as_raw_fd();
    let app = Rc::new(RefCell::new(app));
    let event_app = Rc::clone(&app);
    let wake_app = Rc::clone(&app);
    let identity = X11WindowIdentity {
        instance: "flamewm-assistant".into(),
        class: "flamewm-assistant".into(),
    };
    let (config, role, press_watch) = match anchor {
        None => (
            UiWindowConfig {
                width: WIDTH,
                height: HEIGHT,
                x: centred(root_size, WIDTH, 0),
                y: centred(root_size, HEIGHT, 1),
                title: "FlameWM Assistant".into(),
                identity: Some(identity),
            },
            UiWindowRole::Normal,
            RawPressWatch::default(),
        ),
        Some(anchor) => {
            let size = (WIDTH as i32, window_height as i32);
            let (x, y) = popover::origin(root_size, size, anchor);
            let watch = RawPressWatch::default();
            watch.set(true);
            (
                UiWindowConfig {
                    width: WIDTH,
                    height: window_height,
                    x,
                    y,
                    title: "FlameWM Assistant".into(),
                    identity: Some(identity),
                },
                UiWindowRole::PopupMenu,
                watch,
            )
        }
    };
    let popover_rects = anchor.map(|anchor| {
        let (x, y) = popover::origin(root_size, (WIDTH as i32, window_height as i32), anchor);
        [[x, y, WIDTH as i32, window_height as i32], anchor]
    });
    let popover_open = anchor.is_some();
    run_with_controller_events_role_two_readable_and_presses(
        document,
        config,
        role,
        fd,
        move || drain(&mut wake_reader),
        None,
        press_watch,
        move |press: &RawPress, _doc: &mut flamewm_ui_x11::UiDocumentView<'_>| {
            match popover_rects {
                Some(rects) if popover::is_outside(press, &rects) => {
                    Err(popover::DISMISSED.to_owned())
                }
                _ => Ok(()),
            }
        },
        move |event, doc| {
            let mut app = event_app.borrow_mut();
            match event {
                UiControllerEvent::Key(key)
                    if popover_open
                        && key.phase == UiKeyPhase::Press
                        && key.key == "Escape" =>
                {
                    return Err(popover::DISMISSED.to_owned());
                }
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
    fn api_key_comes_from_settings_or_the_public_default() {
        let mut theme = SettingsSnapshot::new(1);
        assert_eq!(api_key_setting(&theme), DEFAULT_API_KEY_FOR_TESTS);
        theme.values.insert("assistant.apiKey".into(), SettingValue::Text("   ".into()));
        assert_eq!(api_key_setting(&theme), DEFAULT_API_KEY_FOR_TESTS);
        theme.values.insert("assistant.apiKey".into(), SettingValue::Text("ark_mine".into()));
        assert_eq!(api_key_setting(&theme), "ark_mine");
        // A retired Hermes URL setting is never read as a key.
        let mut legacy = SettingsSnapshot::new(1);
        legacy.values.insert("assistant.hermesUrl".into(), SettingValue::Text("http://127.0.0.1:8642".into()));
        assert_eq!(api_key_setting(&legacy), DEFAULT_API_KEY_FOR_TESTS);
    }

    const DEFAULT_API_KEY_FOR_TESTS: &str = flamewm_assistant_core::DEFAULT_API_KEY;

    #[test]
    fn speech_flags_default_off() {
        let mut theme = SettingsSnapshot::new(1);
        assert!(!flag(&theme, "assistant.speechToText"));
        theme.values.insert("assistant.speechToText".into(), SettingValue::Boolean(true));
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
    fn accent_reads_the_settings_hex_text() {
        let mut theme = SettingsSnapshot::new(1);
        theme.values.insert("accent".to_owned(), SettingValue::Text("#EF4048".to_owned()));
        assert_eq!(accent(&theme), 0x00ef_4048);
    }

    #[test]
    fn pretty_model_title_cases_words() {
        assert_eq!(pretty_model("solar-pro"), "Solar Pro");
        assert_eq!(pretty_model("pro"), "Pro");
    }

    #[test]
    fn model_label_and_fit_are_short_and_stable() {
        assert_eq!(model_label("pro"), "pro");
        assert_eq!(model_label("org/long-model-name-here-x"), "long-model-name...");
    }

    #[test]
    fn fallback_list_is_lite_flash_pro() {
        assert_eq!(fallback_models(), vec!["lite", "flash", "pro"]);
    }
}
