//! Typed product-facing boundary between FlameWM UI and the X11 renderer.

pub mod backdrop;
mod event_source;
mod input;
mod startup_protocol;
pub use startup_protocol::{StartupX11Event, StartupX11Observer};
mod popup;
mod root_property;
pub use root_property::RootCardinalPublisher;
mod reconcile;
mod surface_runtime;
mod template;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use event_source::{
    pump_events, run_surface_runtime_until, run_surface_runtime_with_reactor_access,
};
pub use flamewm_render_x11::{
    DockEdge, GeometryTrace, RawPress, RawPressWatch, WindowGeometry, WindowGeometrySlot,
    X11WindowIdentity, apply_cursor_shadow, apply_cursor_theme, safe_file_name, valid_bus_name,
    valid_object_path,
};
pub use input::{
    SurfaceEvent, UiInputEvent, WHEEL_SCROLL_STEP, pointer_button_from_raw, wheel_scroll_delta,
};
pub use popup::PopupState;
pub use reconcile::{reconcile, update};
pub use surface_runtime::{
    SlideConfig, SlideEdge, SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole,
    SurfaceRuntime, UiBackendError,
};
pub use template::{
    RuntimeImage, UiColor, UiDocument, UiDocumentAccess, UiDocumentView, UiTemplate,
    decode_document,
};

use std::os::fd::RawFd;
use std::path::PathBuf;

use calloop::Interest;
use flamewm_reactor::{FdAction, Reactor};

pub use flamewm_render_core::{ActionPhase, Overflow, PointerButton};
pub use flamewm_render_x11::{
    ExternalDrawableSession, ExternalDrawableTarget, SurfaceInputMode as RenderSurfaceInputMode,
    SurfaceRole as RenderSurfaceRole,
};
pub use flamewm_ui::{UiAction, UiEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct UiActionEvent {
    pub action: String,
    pub phase: UiActionPhase,
    pub inside: bool,
    pub button: u32,
    pub modifiers: u32,
    pub x: f32,
    pub y: f32,
    pub text: Option<String>,
}

impl UiActionEvent {
    pub const fn ctrl_down(&self) -> bool {
        self.modifiers & (1 << 2) != 0
    }

    pub const fn shift_down(&self) -> bool {
        self.modifiers & (1 << 0) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKeyPhase {
    Press,
    Release,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiKeyEvent {
    pub phase: UiKeyPhase,
    pub modifiers: Vec<String>,
    pub key: String,
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiActionPhase {
    Press,
    Hover,
    Motion,
    Release,
}

/// Files dragged over, or dropped on, an XDND-enabled window. Positions are window-relative
/// logical px. `Enter`/`Over` carry no paths: the data is only converted on drop.
#[derive(Clone, Debug, PartialEq)]
pub enum UiDragEvent {
    Enter {
        x: f32,
        y: f32,
    },
    Over {
        x: f32,
        y: f32,
    },
    Leave,
    Drop {
        x: f32,
        y: f32,
        /// Local paths for `DropPayload::Uris`; empty for the other payloads.
        paths: Vec<PathBuf>,
        action: UiDragAction,
        payload: DropPayload,
    },
}

/// What a drop carries. `Uris` data is in `Drop::paths`. The Ark payload asks the owner to
/// extract the source's selection into the window's current folder over DBus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropPayload {
    Uris,
    ArkExtract { service: String, path: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiDragAction {
    Copy,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiControllerEvent {
    Action(UiActionEvent),
    Key(UiKeyEvent),
    /// XDND drop-target traffic (only for windows opted in with `with_file_drop_target`).
    Drag(UiDragEvent),
    /// The window's client size settled (logical px). Emitted once per settled size.
    Resized {
        width: u32,
        height: u32,
    },
}

const TYPED_KEY_ACTION: &str = "__flamewm.ui.key";
const KEY_PAYLOAD_SEPARATOR: char = '\u{1f}';

fn drag_event(event: &flamewm_render_core::ActionEvent) -> Option<UiDragEvent> {
    let (x, y) = (event.x, event.y);
    Some(match event.action.as_str() {
        flamewm_render_x11::FILE_DRAG_ENTER_ACTION => UiDragEvent::Enter { x, y },
        flamewm_render_x11::FILE_DRAG_OVER_ACTION => UiDragEvent::Over { x, y },
        flamewm_render_x11::FILE_DRAG_LEAVE_ACTION => UiDragEvent::Leave,
        flamewm_render_x11::FILE_DRAG_DROP_ACTION => UiDragEvent::Drop {
            x,
            y,
            paths: flamewm_render_x11::parse_uri_list(
                event.text.as_deref().unwrap_or("").as_bytes(),
            ),
            action: UiDragAction::Copy,
            payload: DropPayload::Uris,
        },
        flamewm_render_x11::FILE_DRAG_EXTRACT_ACTION => {
            let (service, path) = event
                .text
                .as_deref()
                .and_then(|text| text.split_once('\u{1f}'))
                .unwrap_or_default();
            UiDragEvent::Drop {
                x,
                y,
                paths: Vec::new(),
                action: UiDragAction::Copy,
                payload: DropPayload::ArkExtract {
                    service: service.to_string(),
                    path: path.to_string(),
                },
            }
        }
        _ => return None,
    })
}

fn controller_event(event: &flamewm_render_core::ActionEvent) -> UiControllerEvent {
    if let Some(drag) = drag_event(event) {
        return UiControllerEvent::Drag(drag);
    }
    if event.action == flamewm_render_x11::WINDOW_RESIZE_ACTION {
        return UiControllerEvent::Resized {
            width: event.x.round().max(1.0) as u32,
            height: event.y.round().max(1.0) as u32,
        };
    }
    if event.action == TYPED_KEY_ACTION {
        if let Some(payload) = event.text.as_deref() {
            let mut fields = payload.splitn(3, KEY_PAYLOAD_SEPARATOR);
            let modifiers = fields
                .next()
                .unwrap_or_default()
                .split('+')
                .filter(|modifier| !modifier.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let Some(key) = fields.next().filter(|key| !key.is_empty()) else {
                return UiControllerEvent::Action(action_event(event));
            };
            let text = fields
                .next()
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
            let phase = match event.phase {
                flamewm_render_core::ActionPhase::Press => UiKeyPhase::Press,
                flamewm_render_core::ActionPhase::Release => UiKeyPhase::Release,
                _ => return UiControllerEvent::Action(action_event(event)),
            };
            return UiControllerEvent::Key(UiKeyEvent {
                phase,
                modifiers,
                key: key.to_owned(),
                text,
            });
        }
    }
    UiControllerEvent::Action(action_event(event))
}

fn action_event(action: &flamewm_render_core::ActionEvent) -> UiActionEvent {
    UiActionEvent {
        action: action.action.clone(),
        phase: match action.phase {
            flamewm_render_core::ActionPhase::Press => UiActionPhase::Press,
            flamewm_render_core::ActionPhase::Hover => UiActionPhase::Hover,
            flamewm_render_core::ActionPhase::Motion => UiActionPhase::Motion,
            flamewm_render_core::ActionPhase::Release => UiActionPhase::Release,
        },
        inside: action.inside,
        button: action.button,
        modifiers: action.modifiers,
        x: action.x,
        y: action.y,
        text: action.text.clone(),
    }
}

#[derive(Clone, Debug)]
pub struct UiWindowConfig {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub title: String,
    pub identity: Option<X11WindowIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWindowRole {
    Normal,
    Desktop,
    Dock,
}

fn x11_config(config: UiWindowConfig, role: UiWindowRole) -> flamewm_render_x11::X11Config {
    flamewm_render_x11::X11Config {
        width: config.width,
        height: config.height,
        x: config.x,
        y: config.y,
        title: config.title,
        identity: config.identity,
        backdrop: None,
        file_drag: None,
        role: match role {
            UiWindowRole::Normal => flamewm_render_x11::X11WindowRole::Normal,
            UiWindowRole::Desktop => flamewm_render_x11::X11WindowRole::Desktop,
            UiWindowRole::Dock => flamewm_render_x11::X11WindowRole::Dock,
        },
    }
}

pub fn root_geometry() -> Result<(u32, u32), String> {
    flamewm_render_x11::root_geometry()
}

/// Reactor-backed window loop with a root press observer: while `press_watch` is armed, each root press is delivered to
/// `on_press` (logical px, observed only, never consumed).
pub fn run_with_controller_events_role_with_presses_and_reactor_geometry<F, P, R>(
    document: UiDocument,
    config: UiWindowConfig,
    role: UiWindowRole,
    reactor: &mut Reactor,
    geometry: WindowGeometrySlot,
    press_watch: RawPressWatch,
    mut on_press: P,
    mut on_event: F,
    mut on_reactor: R,
) -> Result<(), String>
where
    F: for<'a> FnMut(&UiControllerEvent, &mut UiDocumentView<'a>) -> Result<(), String>,
    P: for<'a> FnMut(&RawPress, &mut UiDocumentView<'a>) -> Result<(), String>,
    R: for<'a> FnMut(&mut UiDocumentView<'a>) -> Result<(), String>,
{
    let backdrop = document.window_backdrop;
    let file_drag = document
        .file_drop_target
        .then(flamewm_render_x11::FileDragSlot::default);
    let mut x11 = x11_config(config, role);
    x11.backdrop = backdrop;
    x11.file_drag = file_drag.clone();
    let press_drag = file_drag.clone();
    let reactor_drag = file_drag.clone();
    flamewm_render_x11::run_with_controller_events_with_reactor_geometry(
        document.document,
        x11,
        reactor,
        geometry,
        press_watch,
        move |event, document| {
            let flamewm_render_core::ControllerEvent::Action(action) = event;
            let event = controller_event(action);
            on_event(
                &event,
                &mut UiDocumentView {
                    document,
                    file_drag: file_drag.clone(),
                },
            )
        },
        move |press, document| {
            on_press(
                press,
                &mut UiDocumentView {
                    document,
                    file_drag: press_drag.clone(),
                },
            )
        },
        move |document| {
            on_reactor(&mut UiDocumentView {
                document,
                file_drag: reactor_drag.clone(),
            })
        },
    )
}

/// Whether a caller-owned readable descriptor stays registered after one drain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWakeDrain {
    /// Keep watching the descriptor.
    Keep,
    /// Stop watching. The caller has already queued the failure for `on_wake`.
    Retire,
}

fn wake_action(drain: UiWakeDrain) -> FdAction {
    match drain {
        UiWakeDrain::Keep => FdAction::Continue,
        UiWakeDrain::Retire => FdAction::Remove,
    }
}

/// Same reactor-backed window loop with a second independent readable wake source.
/// Both drains only queue data; document access stays in `on_wake`.
pub fn run_with_controller_events_role_and_two_readable_fds<F, W, D>(
    document: UiDocument,
    config: UiWindowConfig,
    role: UiWindowRole,
    readable_fd: RawFd,
    mut drain: D,
    second_readable: Option<(RawFd, Box<dyn FnMut() -> UiWakeDrain>)>,
    on_event: F,
    on_wake: W,
) -> Result<(), String>
where
    F: for<'a> FnMut(&UiControllerEvent, &mut UiDocumentView<'a>) -> Result<(), String>,
    W: for<'a> FnMut(&mut UiDocumentView<'a>) -> Result<(), String>,
    D: FnMut() -> UiWakeDrain + 'static,
{
    let mut reactor = Reactor::new().map_err(|error| error.to_string())?;
    // Level-triggered registration: a retired source is removed by the reactor, and the reactor
    // itself is dropped when this function returns, so no explicit removal is needed.
    let _registration =
        reactor
            .register_raw_fd_with_action(readable_fd, Interest::READ, move |_, _| {
                wake_action(drain())
            })
            .map_err(|error| error.to_string())?;
    let _second_registration = second_readable
        .map(|(fd, mut second_drain)| {
            reactor
                .register_raw_fd_with_action(fd, Interest::READ, move |_, _| {
                    wake_action(second_drain())
                })
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    run_with_controller_events_role_with_presses_and_reactor_geometry(
        document,
        config,
        role,
        &mut reactor,
        WindowGeometrySlot::default(),
        RawPressWatch::default(),
        |_, _| Ok(()),
        on_event,
        on_wake,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn ctrl_a_arrives_as_key_a_with_ctrl_and_no_text() {
        let typed = flamewm_render_core::ActionEvent {
            action: super::TYPED_KEY_ACTION.into(),
            phase: flamewm_render_core::ActionPhase::Press,
            button: 0,
            modifiers: 0,
            x: 0.0,
            y: 0.0,
            inside: true,
            time_ms: 0,
            text: Some("Ctrl\u{1f}a\u{1f}".into()),
        };
        assert_eq!(
            super::controller_event(&typed),
            super::UiControllerEvent::Key(super::UiKeyEvent {
                phase: super::UiKeyPhase::Press,
                modifiers: vec!["Ctrl".into()],
                key: "a".into(),
                text: None,
            })
        );
    }

    #[test]
    fn settled_window_size_maps_to_resized_event_not_action() {
        let resize = flamewm_render_core::ActionEvent {
            action: flamewm_render_x11::WINDOW_RESIZE_ACTION.into(),
            phase: flamewm_render_core::ActionPhase::Release,
            button: 0,
            modifiers: 0,
            x: 1024.0,
            y: 0.0,
            inside: true,
            time_ms: 0,
            text: None,
        };
        assert_eq!(
            super::controller_event(&resize),
            super::UiControllerEvent::Resized {
                width: 1024,
                height: 1
            },
            "a zero height clamps to 1 so the window never reports an empty size"
        );
    }

    #[test]
    fn ui_action_modifiers_copy_event_state_through() {
        let core = flamewm_render_core::ActionEvent {
            action: "explorer.row.0".into(),
            phase: flamewm_render_core::ActionPhase::Release,
            button: 1,
            modifiers: (1 << 0) | (1 << 2),
            x: 10.0,
            y: 25.0,
            inside: true,
            time_ms: 0,
            text: None,
        };
        let action = super::action_event(&core);
        assert!(action.ctrl_down());
        assert!(action.shift_down());
        assert!(
            !super::UiActionEvent {
                modifiers: 0,
                ..action.clone()
            }
            .ctrl_down()
        );
    }

    #[test]
    fn readable_fd_seam_is_a_reactor_source_not_a_timer() {
        let source = include_str!("lib.rs");
        let production = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("ui production source");
        assert!(
            production.contains("register_raw_fd_with_action(readable_fd"),
            "external wake descriptors must enter through the reactor"
        );
        assert!(
            !production.contains("register_timer"),
            "external wake delivery must not use timers"
        );
    }
}
