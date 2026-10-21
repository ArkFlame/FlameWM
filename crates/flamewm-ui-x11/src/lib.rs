//! Typed product-facing boundary between FlameWM UI and the X11 renderer.

pub mod backdrop;
mod event_source;
mod input;
mod startup_protocol;
pub use startup_protocol::{StartupX11Event, StartupX11Observer};
mod popup;
mod reconcile;
mod surface_runtime;
mod template;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use event_source::{
    pump_events, run_surface_runtime_until, run_surface_runtime_with_reactor,
    run_surface_runtime_with_reactor_access,
};
pub use flamewm_render_x11::{
    DockEdge, GeometryTrace, RawPress, RawPressWatch, WindowGeometry, WindowGeometrySlot,
    X11WindowIdentity, apply_cursor_shadow, apply_cursor_theme,
};
pub use input::{
    SurfaceEvent, UiInputEvent, WHEEL_SCROLL_STEP, pointer_button_from_raw,
    surface_event_to_ui_event, wheel_scroll_delta,
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

#[derive(Clone, Debug, PartialEq)]
pub enum UiControllerEvent {
    Action(UiActionEvent),
    Key(UiKeyEvent),
}

const TYPED_KEY_ACTION: &str = "__flamewm.ui.key";
const KEY_PAYLOAD_SEPARATOR: char = '\u{1f}';

fn controller_event(event: &flamewm_render_core::ActionEvent) -> UiControllerEvent {
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
    flamewm_render_x11::run_with_controller_events_with_reactor_geometry(
        document.document,
        x11_config(config, role),
        reactor,
        geometry,
        press_watch,
        move |event, document| {
            let flamewm_render_core::ControllerEvent::Action(action) = event;
            let event = controller_event(action);
            on_event(&event, &mut UiDocumentView { document })
        },
        move |press, document| on_press(press, &mut UiDocumentView { document }),
        move |document| on_reactor(&mut UiDocumentView { document }),
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
    let _registration = reactor
        .register_raw_fd_with_action(readable_fd, Interest::READ, move |_, _| match drain() {
            UiWakeDrain::Keep => FdAction::Continue,
            UiWakeDrain::Retire => FdAction::Remove,
        })
        .map_err(|error| error.to_string())?;
    let _second_registration = second_readable
        .map(|(fd, mut second_drain)| {
            reactor
                .register_raw_fd_with_action(fd, Interest::READ, move |_, _| match second_drain() {
                    UiWakeDrain::Keep => FdAction::Continue,
                    UiWakeDrain::Retire => FdAction::Remove,
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
        assert!(!super::UiActionEvent { modifiers: 0, ..action.clone() }.ctrl_down());
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
