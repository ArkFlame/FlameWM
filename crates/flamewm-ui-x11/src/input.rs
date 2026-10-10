use flamewm_render_core::{ActionEvent, ActionPhase, PointerButton, ScrollDelta};

use flamewm_ui::{ActionId, UiEvent, WidgetId};

/// Translate a raw X button once at the native boundary.
pub fn pointer_button_from_raw(button: u32) -> PointerButton {
    PointerButton::from_raw_x(button)
}

/// Semantic vertical scroll step for Button4/5 wheel ticks.
pub const WHEEL_SCROLL_STEP: f32 = 48.0;

/// Map a wheel button to its semantic scroll delta. Returns None for
/// non-wheel buttons.
pub fn wheel_scroll_delta(button: PointerButton) -> Option<ScrollDelta> {
    match button {
        PointerButton::WheelUp => Some(ScrollDelta {
            dx: 0.0,
            dy: -WHEEL_SCROLL_STEP,
        }),
        PointerButton::WheelDown => Some(ScrollDelta {
            dx: 0.0,
            dy: WHEEL_SCROLL_STEP,
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceEvent {
    pub surface: crate::SurfaceHandle,
    pub action: ActionEvent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiInputEvent {
    pub surface: crate::SurfaceHandle,
    pub action: ActionEvent,
}

pub fn surface_event_to_ui_event(event: &SurfaceEvent) -> Option<UiEvent> {
    action_to_ui_event(&event.action)
}

pub(crate) fn action_to_ui_event(event: &ActionEvent) -> Option<UiEvent> {
    if event.phase != ActionPhase::Release || !event.inside {
        return None;
    }
    if let Some(text) = event.text.clone() {
        return Some(UiEvent::TextChanged {
            widget: WidgetId::new(event.action.clone()),
            text,
        });
    }
    Some(UiEvent::Activate {
        widget: WidgetId::new(event.action.clone()),
        action: ActionId::new(event.action.clone()),
    })
}
