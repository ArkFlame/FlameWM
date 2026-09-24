//! Typed product-facing boundary between FlameWM UI and the X11 renderer.

mod event_source;
mod input;
mod popup;
mod reconcile;
mod surface_runtime;
mod template;

pub use event_source::{
    pump_events, run_surface_runtime_with_reactor, run_surface_runtime_with_reactor_access,
};
pub use flamewm_render_x11::GeometryTrace;
pub use input::{
    SurfaceEvent, UiInputEvent, WHEEL_SCROLL_STEP, action_scroll_delta, pointer_button_from_raw,
    surface_event_to_ui_event, wheel_scroll_delta,
};
pub use popup::PopupState;
pub use reconcile::{reconcile, update};
pub use surface_runtime::{
    SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError,
};
pub use template::{
    RuntimeImage, UiColor, UiDocument, UiDocumentAccess, UiDocumentView, UiTemplate,
    decode_document,
};

use flamewm_reactor::Reactor;

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
    pub x: f32,
    pub y: f32,
    pub text: Option<String>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWindowRole {
    Normal,
    Desktop,
    Dock,
}

pub fn root_geometry() -> Result<(u32, u32), String> {
    flamewm_render_x11::root_geometry()
}

pub fn run_with_controller_events<F>(
    document: UiDocument,
    config: UiWindowConfig,
    on_event: F,
) -> Result<(), String>
where
    F: for<'a> FnMut(&UiControllerEvent, &mut UiDocumentView<'a>) -> Result<(), String>,
{
    run_with_controller_events_role(document, config, UiWindowRole::Normal, on_event)
}

pub fn run_with_controller_events_role<F>(
    document: UiDocument,
    config: UiWindowConfig,
    role: UiWindowRole,
    mut on_event: F,
) -> Result<(), String>
where
    F: for<'a> FnMut(&UiControllerEvent, &mut UiDocumentView<'a>) -> Result<(), String>,
{
    flamewm_render_x11::run_with_controller_events(
        document.document,
        flamewm_render_x11::X11Config {
            width: config.width,
            height: config.height,
            x: config.x,
            y: config.y,
            title: config.title,
            role: match role {
                UiWindowRole::Normal => flamewm_render_x11::X11WindowRole::Normal,
                UiWindowRole::Desktop => flamewm_render_x11::X11WindowRole::Desktop,
                UiWindowRole::Dock => flamewm_render_x11::X11WindowRole::Dock,
            },
        },
        move |event, document| {
            let flamewm_render_core::ControllerEvent::Action(action) = event;
            let event = controller_event(action);
            on_event(&event, &mut UiDocumentView { document })
        },
    )
}

pub fn run_with_controller_events_role_with_reactor<F, R>(
    document: UiDocument,
    config: UiWindowConfig,
    role: UiWindowRole,
    reactor: &mut Reactor,
    mut on_event: F,
    mut on_reactor: R,
) -> Result<(), String>
where
    F: for<'a> FnMut(&UiControllerEvent, &mut UiDocumentView<'a>) -> Result<(), String>,
    R: for<'a> FnMut(&mut UiDocumentView<'a>) -> Result<(), String>,
{
    flamewm_render_x11::run_with_controller_events_with_reactor(
        document.document,
        flamewm_render_x11::X11Config {
            width: config.width,
            height: config.height,
            x: config.x,
            y: config.y,
            title: config.title,
            role: match role {
                UiWindowRole::Normal => flamewm_render_x11::X11WindowRole::Normal,
                UiWindowRole::Desktop => flamewm_render_x11::X11WindowRole::Desktop,
                UiWindowRole::Dock => flamewm_render_x11::X11WindowRole::Dock,
            },
        },
        reactor,
        move |event, document| {
            let flamewm_render_core::ControllerEvent::Action(action) = event;
            let event = controller_event(action);
            on_event(&event, &mut UiDocumentView { document })
        },
        move |document| on_reactor(&mut UiDocumentView { document }),
    )
}
