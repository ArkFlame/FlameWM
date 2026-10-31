use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::settings::{
    SettingValue, SettingsChange, SettingsSnapshot, SettingsTransaction,
};
use flamewm_api::shortcuts::ShortcutSnapshot;
use flamewm_api::{ModeId, OutputId, Rect, Size};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_settings_core::{ControlTransport, SettingsClient};
use flamewm_ui_x11::{RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess};

use crate::SettingsApplication;

mod markup_errors;
mod taskbar_about;
mod hotkeys;
mod appearance;
mod animation_lock;
mod taskbar_screensaver;
mod stepper_controls;
mod segments_backgrounds;
mod weather;
mod toggles_and_seams;
mod accent_color;
mod api_key;
mod color_picker;
mod hotkey_seams;
mod display_modes;
mod display_paging;
mod page_guards;

#[derive(Default)]
struct RecordingTransport {
    responses: VecDeque<Result<ControlResponse, flamewm_control_core::ControlError>>,
    requests: Rc<RefCell<Vec<ControlRequest>>>,
}

fn recording_transport() -> (Rc<RefCell<Vec<ControlRequest>>>, RecordingTransport) {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        requests: requests.clone(),
        ..RecordingTransport::default()
    };
    (requests, transport)
}

/// `(key, value)` of every `ApplySettings` write, in request order.
fn applied_writes(requests: &RefCell<Vec<ControlRequest>>) -> Vec<(String, SettingValue)> {
    requests
        .borrow()
        .iter()
        .filter_map(|request| match request {
            ControlRequest::ApplySettings(transaction) => {
                transaction.changes.first().and_then(|change| {
                    change
                        .value
                        .clone()
                        .map(|value| (change.key.clone(), value))
                })
            }
            _ => None,
        })
        .collect()
}

fn queue_displays_shortcuts_panels(transport: &mut RecordingTransport) {
    transport.responses.extend([
        Ok(ControlResponse::Displays(display_snapshot())),
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 1,
            bindings: BTreeMap::new(),
        })),
        Ok(ControlResponse::Panels(
            flamewm_api::panels::PanelsSnapshot {
                revision: 1,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            },
        )),
    ]);
}

impl ControlTransport for RecordingTransport {
    fn call(
        &mut self,
        request: ControlRequest,
    ) -> Result<ControlResponse, flamewm_control_core::ControlError> {
        self.requests.borrow_mut().push(request);
        self.responses
            .pop_front()
            .expect("controller test response was not queued")
    }
}

/// Controller over `transport`, with the queued snapshot loaded by `refresh`.
fn loaded_application<R, E: std::fmt::Debug>(
    transport: RecordingTransport,
    refresh: impl FnOnce(&mut SettingsClient<RecordingTransport>) -> Result<R, E>,
) -> SettingsApplication<RecordingTransport> {
    let mut application = SettingsApplication::new(transport);
    refresh(application.client_mut()).expect("queued snapshot");
    application
}

/// Controller over `transport`, with the queued settings snapshot already loaded.
fn settings_application(
    transport: RecordingTransport,
) -> SettingsApplication<RecordingTransport> {
    loaded_application(
        transport,
        flamewm_settings_core::SettingsClient::refresh_settings,
    )
}

/// Controller over `transport`, with the queued display snapshot already loaded.
fn displays_application(
    transport: RecordingTransport,
) -> SettingsApplication<RecordingTransport> {
    loaded_application(
        transport,
        flamewm_settings_core::SettingsClient::refresh_displays,
    )
}

/// Controller over `transport`, with the queued shortcut snapshot already loaded.
fn shortcuts_application(
    transport: RecordingTransport,
) -> SettingsApplication<RecordingTransport> {
    loaded_application(
        transport,
        flamewm_settings_core::SettingsClient::refresh_shortcuts,
    )
}

/// Values written to `key` by `ApplySettings` requests, in request order.
fn written_values(requests: &RefCell<Vec<ControlRequest>>, key: &str) -> Vec<SettingValue> {
    applied_writes(requests)
        .into_iter()
        .filter(|(written, _)| written == key)
        .map(|(_, value)| value)
        .collect()
}

#[derive(Default)]
struct ProbeDocument {
    texts: BTreeMap<String, String>,
    visible: BTreeMap<String, bool>,
    highlighted: BTreeMap<String, bool>,
}

impl UiDocumentAccess for ProbeDocument {
    fn color_variable(&mut self, _name: &str, _color: UiColor) -> Result<(), String> {
        Ok(())
    }

    fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        crate::ui_contract::require_markup_id(id)?;
        self.texts.insert(id.to_owned(), text);
        Ok(())
    }

    fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        crate::ui_contract::require_markup_id(id)?;
        self.visible.insert(id.to_owned(), visible);
        Ok(())
    }

    fn toggle(
        &mut self,
        _track_id: &str,
        _knob_id: &str,
        _checked: bool,
    ) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_track_id)?;
        crate::ui_contract::require_markup_id(_knob_id)
    }

    fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn background(&mut self, id: &str, _color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(id)?;
        self.highlighted.insert(id.to_owned(), true);
        Ok(())
    }

    fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(id)?;
        self.highlighted.insert(id.to_owned(), false);
        Ok(())
    }

    fn border_clear(&mut self, _id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn font_size(&mut self, _id: &str, _size_px: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn font_weight(&mut self, _id: &str, _weight: u16) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn overflow(
        &mut self,
        _id: &str,
        _x: flamewm_ui_x11::Overflow,
        _y: flamewm_ui_x11::Overflow,
    ) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn layer(
        &mut self,
        _id: &str,
        _layer: flamewm_ui_core::style::UiLayer,
    ) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }
}

fn release(action: &str) -> UiActionEvent {
    UiActionEvent {
        action: action.to_owned(),
        phase: UiActionPhase::Release,
        inside: true,
        button: 1,
        modifiers: 0,
        x: 0.0,
        y: 0.0,
        text: None,
    }
}

fn display_snapshot() -> DisplaySnapshot {
    let mode = ModeId(7);
    DisplaySnapshot {
        generation: 3,
        outputs: vec![OutputSnapshot {
            id: OutputId::new("HDMI-1"),
            connector: "HDMI-1".to_owned(),
            edid_identity: "test-edid".to_owned(),
            connected: true,
            primary: true,
            geometry: Rect::new(0, 0, 1920, 1080),
            current_mode: mode,
            modes: vec![DisplayMode {
                id: mode,
                resolution: Size::new(1920, 1080),
                refresh_millihz: 60_000,
                preferred: true,
            }],
            shell_scale_percent: 100,
        }],
        pending: None,
    }
}

fn markup_actions() -> Vec<&'static str> {
    include_str!("../../../../ui/settings.html")
        .split("data-action=\"")
        .skip(1)
        .filter_map(|fragment| fragment.split('"').next())
        .collect()
}

/// A session whose snapshot holds `initial` for the surface colours. One queued
/// response per expected `ApplySettings` call keeps the transport in step.
fn color_session(
    initial: &str,
    applies: usize,
) -> (
    SettingsApplication<RecordingTransport>,
    Rc<RefCell<Vec<ControlRequest>>>,
) {
    let (requests, mut transport) = recording_transport();
    let mut snapshot = SettingsSnapshot::new(7);
    for key in [
        "surfaceColor",
        "surfaceBorderColor",
        "lockBackgroundColor",
        "accent",
    ] {
        snapshot
            .values
            .insert(key.to_owned(), SettingValue::Text(initial.to_owned()));
    }
    for _ in 0..=applies {
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(snapshot.clone())));
    }
    let application = settings_application(transport);
    (application, requests)
}

fn typed(text: &str) -> UiActionEvent {
    UiActionEvent {
        text: Some(text.to_owned()),
        ..release("keyboard.input")
    }
}

fn last_apply(requests: &Rc<RefCell<Vec<ControlRequest>>>, key: &str) -> Option<String> {
    match requests.borrow().last() {
        Some(ControlRequest::ApplySettings(transaction)) => transaction
            .changes
            .iter()
            .find(|change| change.key == key)
            .and_then(|change| match &change.value {
                Some(SettingValue::Text(value)) => Some(value.clone()),
                _ => None,
            }),
        _ => None,
    }
}

fn lock_snapshot(revision: u64, key: &str, value: SettingValue) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    snapshot.values.insert(key.to_owned(), value);
    snapshot
}

fn decoration_snapshot(revision: u64, decoration: &str) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    snapshot.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#EF4048".to_owned()),
    );
    snapshot.values.insert(
        "decoration".to_owned(),
        SettingValue::Text(decoration.to_owned()),
    );
    snapshot
}

fn two_output_display_snapshot() -> DisplaySnapshot {
    let mut snapshot = two_mode_display_snapshot();
    let mut second = snapshot.outputs[0].clone();
    second.id = OutputId::new("DP-1");
    second.connector = "DP-1".to_owned();
    second.primary = false;
    second.current_mode = ModeId(8);
    second.geometry = Rect::new(1920, 0, 1280, 720);
    snapshot.outputs.push(second);
    snapshot
}

/// Twelve modes with ids 0..12 on the selected output, paged four at a time.
fn twelve_mode_display_snapshot() -> DisplaySnapshot {
    let mut snapshot = display_snapshot();
    let output = &mut snapshot.outputs[0];
    output.modes = (0..12u64)
        .map(|id| DisplayMode {
            id: ModeId(id),
            resolution: Size::new(640, 480),
            refresh_millihz: 60_000,
            preferred: id == 0,
        })
        .collect();
    output.current_mode = ModeId(0);
    snapshot
}

fn begun_modes(requests: &Rc<RefCell<Vec<ControlRequest>>>) -> Vec<ModeId> {
    requests
        .borrow()
        .iter()
        .filter_map(|request| match request {
            ControlRequest::BeginDisplayMode { mode, .. } => Some(*mode),
            _ => None,
        })
        .collect()
}

fn shortcut_snapshot_with(bindings: &[(&str, &str)]) -> ShortcutSnapshot {
    ShortcutSnapshot {
        revision: 7,
        bindings: bindings
            .iter()
            .map(|(action, key)| {
                (
                    (*action).to_owned(),
                    flamewm_api::shortcuts::KeyBinding(Some((*key).to_owned())),
                )
            })
            .collect(),
    }
}

fn two_mode_display_snapshot() -> DisplaySnapshot {
    let mut snapshot = display_snapshot();
    let output = &mut snapshot.outputs[0];
    output.modes = vec![
        DisplayMode {
            id: ModeId(7),
            resolution: Size::new(1920, 1080),
            refresh_millihz: 60_000,
            preferred: true,
        },
        DisplayMode {
            id: ModeId(8),
            resolution: Size::new(1280, 720),
            refresh_millihz: 50_000,
            preferred: false,
        },
    ];
    snapshot
}
