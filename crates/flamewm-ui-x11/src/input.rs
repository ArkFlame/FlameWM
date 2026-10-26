use flamewm_render_core::{ActionEvent, PointerButton, ScrollDelta};

/// Translate a raw X button once at the native boundary.
#[must_use]
pub fn pointer_button_from_raw(button: u32) -> PointerButton {
    PointerButton::from_raw_x(button)
}

/// Semantic vertical scroll step for Button4/5 wheel ticks.
pub const WHEEL_SCROLL_STEP: f32 = 48.0;

/// Map a wheel button to its semantic scroll delta. Returns None for
/// non-wheel buttons.
#[must_use]
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
