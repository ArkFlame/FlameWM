//! Native `FlameWM` assistant: one chat window over ARK AI.
//!
//! The window owns presentation, paging, voice and worker threads. Chat requests go through the
//! ARK AI client in `flamewm-assistant-core`. Blocking work runs on short-lived threads that report
//! back over a channel plus a wake pipe that the window's event loop reads.

mod chat_header;
mod instance;
mod model;
pub mod osd;
pub mod osd_host;
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
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_assistant_core::{
    apply_defaults, default_config_path, default_voice_dir, expand, human_error, key_or_default, ready_model,
    start_recording, ArkClient, ArkConfig, AssistantError, CancelToken, ChatStore, CommandSpeech, CommandTranscriber,
    Message, MsgState, Recording, Role, SessionState, Speech, SpeechConfig, Stopper, SystemRunner, Transcriber, Turn,
    DEFAULT_MODEL, FALLBACK_MODELS, HISTORY_TURNS,
};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiKeyEvent, UiKeyPhase,
    UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document,
    run_with_controller_events_role_and_two_readable_fds,
};

use model::{History, KeyCmd, Row, SLOTS};

/// Rows in the model picker; longer lists page with prev and next.
const MODEL_ROWS: usize = 8;
const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-assistant.rwr"));
const WIDTH: u32 = 460;
const HEIGHT: u32 = 680;
/// Wheel notches move the viewport by this many messages.
const WHEEL_STEP: isize = 3;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x0053_9bff;
const KEY_TINT: u32 = 0x00ff_ffff;

/// Messages from worker threads to the window.
enum Msg {
    /// Model ids from `GET /models`, or the reason ARK AI could not be reached.
    Checked(Result<Vec<String>, AssistantError>),
    /// One streamed text piece of the running reply.
    Delta(String),
    /// The reply finished with its full text, or ended with the reason it did not.
    TurnDone(Result<String, AssistantError>),
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

/// Whether ARK AI answered the last check or a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    Checking,
    Online,
    Offline,
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
    link: Link,
    /// Model ids for the picker: the server's list, or the built-in fallback when it failed.
    models: Vec<String>,
    /// Model chosen in the picker; `None` means the default model.
    model: Option<String>,
    model_menu: bool,
    model_page: usize,
    /// Typed filter for the model picker; the keyboard edits it while the menu is open.
    model_query: String,
    store: ChatStore,
    history: History,
    input: String,
    working: bool,
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
        let saved_model = store.session().model.clone();
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
            link: Link::Checking,
            models: fallback_models(),
            model: saved_model,
            model_menu: false,
            model_page: 0,
            model_query: String::new(),
            store,
            history,
            input: String::new(),
            working: false,
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

    fn ark_config(&self) -> ArkConfig {
        ArkConfig { api_key: self.api_key.clone(), ..ArkConfig::default() }
    }

    /// Lists the models on a worker thread. The result tells the window whether ARK AI answered.
    fn check(&mut self) {
        self.link = Link::Checking;
        let config = self.ark_config();
        let bus = self.bus.clone();
        thread::spawn(move || {
            let result = ArkClient::new(config).and_then(|client| client.list_models());
            bus.send(Msg::Checked(result));
        });
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Checked(Ok(list)) => {
                self.link = Link::Online;
                self.models = list;
                self.notice.clear();
            }
            Msg::Checked(Err(error)) => {
                self.link = Link::Offline;
                self.models = fallback_models();
                self.notice = human_error(&error);
            }
            Msg::Delta(text) => self.history.append_pending(&text),
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
        self.cancel = CancelToken::new();
        if std::mem::take(&mut self.voice_turn) {
            self.set_osd(OsdState::Working);
        }
        self.history.set_pending(Some(String::new()));
        let config = self.ark_config();
        let model = self.model.clone().unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let bus = self.bus.clone();
        let cancel = self.cancel.clone();
        thread::spawn(move || turn_job(&bus, config, &model, &turns, &cancel));
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

    fn finish_turn(&mut self, result: Result<String, AssistantError>) {
        self.working = false;
        let partial = self.history.take_pending().unwrap_or_default();
        match result {
            Ok(text) => {
                self.link = Link::Online;
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
                if matches!(error, AssistantError::Unreachable(_) | AssistantError::Timeout) {
                    self.link = Link::Offline;
                }
                self.notice = human_error(&error);
            }
        }
        if !self.speaking {
            self.set_osd(OsdState::Hidden);
        }
    }

    /// Models shown in the picker: the ids matching the typed filter, as indices into `models`.
    fn model_matches(&self) -> Vec<usize> {
        model::filter_models(&self.models, &self.model_query)
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
        let matches = self.model_matches();
        let position = self
            .model
            .as_ref()
            .and_then(|m| matches.iter().position(|&i| self.models[i] == *m));
        self.model_page = position.map_or(0, |p| p / MODEL_ROWS);
    }

    /// Makes the row `slot` of the current page the model used from the next turn on, and saves it.
    fn select_model(&mut self, slot: usize) {
        let matches = self.model_matches();
        let Some(&index) = matches.get(self.model_page * MODEL_ROWS + slot) else { return };
        self.model = Some(self.models[index].clone());
        self.model_menu = false;
        let state = SessionState { model: self.model.clone() };
        if let Err(error) = self.store.set_session(state) {
            self.notice = format!("Could not save the model: {error}");
        }
    }

    /// Deletes the saved chat after the user confirmed it. The model choice is kept.
    fn clear_chat(&mut self) {
        self.clear_confirm = false;
        if self.working {
            self.notice = "Stop the reply before clearing the chat.".into();
            return;
        }
        let model = self.model.clone();
        let cleared = self.store.clear().and_then(|()| self.store.set_session(SessionState { model }));
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
            "assistant.retry" => self.check(),
            "assistant.send" => self.submit(),
            "assistant.model-toggle" => self.toggle_model_menu(),
            "assistant.model-prev" => self.model_page = self.model_page.saturating_sub(1),
            "assistant.model-next" => {
                let last = self.model_matches().len().saturating_sub(1) / MODEL_ROWS;
                self.model_page = (self.model_page + 1).min(last);
            }
            action if action.starts_with("assistant.model.") => {
                if let Ok(slot) = action["assistant.model.".len()..].parse::<usize>() {
                    self.select_model(slot);
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
                KeyCmd::Submit => self.select_model(0),
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
        doc.text("model", model_label(self.model.as_deref()))?;
        let open = self.model_menu;
        doc.visible("model-menu", open)?;
        let matches = self.model_matches();
        let empty = open && matches.is_empty();
        doc.visible("model-empty", empty)?;
        doc.visible("model-hint", open && self.model_query.is_empty())?;
        doc.text("model-query", self.model_query.clone())?;
        let pages = matches.len().div_ceil(MODEL_ROWS).max(1);
        doc.text("model-page", format!("{} / {}", self.model_page + 1, pages))?;
        for slot in 0..MODEL_ROWS {
            let index = self.model_page * MODEL_ROWS + slot;
            let id = matches.get(index).map(|&i| &self.models[i]);
            doc.visible(&format!("mrow-{slot}"), open && id.is_some())?;
            if let Some(id) = id {
                let current = self.model.as_deref().unwrap_or(DEFAULT_MODEL) == id.as_str();
                doc.text(&format!("mtext-{slot}"), id.clone())?;
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
        doc.text("subtitle", chat_header::subtitle(self.link, self.working).to_owned())?;
        doc.visible("retry", self.link == Link::Offline)?;
        let mic = self.speech_in && empty && !self.working;
        self.paint_models(doc)?;
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

/// The `assistant.apiKey` setting, or the public default when it is missing or blank. A stored
/// value that is not a valid key is kept, so the request reports why it fails.
fn api_key_setting(theme: &SettingsSnapshot) -> String {
    match theme.values.get("assistant.apiKey") {
        Some(SettingValue::Text(key)) => key_or_default(Some(key)),
        _ => key_or_default(None),
    }
}

/// Short header label for a model id: the part after the last `/`, cut to 18 characters.
fn model_label(model: Option<&str>) -> String {
    let Some(id) = model else { return DEFAULT_MODEL.into() };
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
        _ => ACCENT_RGB,
    }
}

/// One streamed reply. Each text piece and the final result go back to the window.
fn turn_job(bus: &Bus, config: ArkConfig, model: &str, turns: &[Turn], cancel: &CancelToken) {
    let result = ArkClient::new(config)
        .and_then(|client| client.chat(model, turns, cancel, &mut |delta| bus.send(Msg::Delta(delta.to_owned()))));
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
    let result = show(state_dir);
    instance::release(&pid_file);
    result
}

fn show(state_dir: PathBuf) -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let store = ChatStore::open(&state_dir).map_err(|error| error.to_string())?;

    let root_size = flamewm_ui_x11::root_geometry().unwrap_or((1280, 800));
    let (mut wake_reader, wake_writer) = nonblocking_pair()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let bus = Bus { tx, wake: Arc::new(wake_writer) };
    let mut app = Assistant::new(state_dir, bus, theme, store)?;
    app.paint(&mut document)?;
    app.check();

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
    fn model_label_and_fit_are_short_and_stable() {
        assert_eq!(model_label(None), DEFAULT_MODEL);
        assert_eq!(model_label(Some("pro")), "pro");
        assert_eq!(model_label(Some("org/long-model-name-here-x")), "long-model-name...");
    }

    #[test]
    fn fallback_list_is_lite_flash_pro() {
        assert_eq!(fallback_models(), vec!["lite", "flash", "pro"]);
    }
}
