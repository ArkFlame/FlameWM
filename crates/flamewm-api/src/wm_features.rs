//! Engine-neutral window-manager capability contracts owned by `FlameWM`.
//!
//! These contracts model mature EWMH/ICCCM behaviors that the executable engine already owns.
//! `FlameWM` may expose them through product UX, but must never create a second authoritative
//! stacking/show-desktop/moveresize state machine above the engine.

use crate::{FlameError, FlameResult, Point, WindowRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFeature {
    Sticky,
    Modal,
    Shaded,
    SkipTaskbar,
    SkipPager,
    Above,
    Below,
    DemandsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAction {
    Add,
    Remove,
    Toggle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowFeatureSnapshot {
    pub sticky: bool,
    pub modal: bool,
    pub shaded: bool,
    pub skip_taskbar: bool,
    pub skip_pager: bool,
    pub above: bool,
    pub below: bool,
    pub demands_attention: bool,
}

impl WindowFeatureSnapshot {
    #[must_use]
    pub const fn enabled(self, feature: WindowFeature) -> bool {
        match feature {
            WindowFeature::Sticky => self.sticky,
            WindowFeature::Modal => self.modal,
            WindowFeature::Shaded => self.shaded,
            WindowFeature::SkipTaskbar => self.skip_taskbar,
            WindowFeature::SkipPager => self.skip_pager,
            WindowFeature::Above => self.above,
            WindowFeature::Below => self.below,
            WindowFeature::DemandsAttention => self.demands_attention,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestackMode {
    Above,
    Below,
    TopIf,
    BottomIf,
    Opposite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenMonitorSpan {
    pub top: usize,
    pub bottom: usize,
    pub left: usize,
    pub right: usize,
}

impl FullscreenMonitorSpan {
    #[must_use]
    pub const fn new(top: usize, bottom: usize, left: usize, right: usize) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveResizeDirection {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Move,
    SizeKeyboard,
    MoveKeyboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractiveMoveResize {
    pub window: WindowRef,
    pub direction: MoveResizeDirection,
    pub root_position: Option<Point>,
    pub button: u32,
}

impl InteractiveMoveResize {
    pub fn validate(self) -> FlameResult<Self> {
        if !self.window.is_valid() {
            return Err(FlameError::invalid(
                "interactive moveresize requires a valid WindowRef",
            ));
        }
        let pointer_direction = matches!(
            self.direction,
            MoveResizeDirection::TopLeft
                | MoveResizeDirection::Top
                | MoveResizeDirection::TopRight
                | MoveResizeDirection::Right
                | MoveResizeDirection::BottomRight
                | MoveResizeDirection::Bottom
                | MoveResizeDirection::BottomLeft
                | MoveResizeDirection::Left
                | MoveResizeDirection::Move
        );
        if pointer_direction && self.root_position.is_none() {
            return Err(FlameError::invalid(
                "pointer moveresize requires a root position",
            ));
        }
        if !pointer_direction && self.button != 0 {
            return Err(FlameError::invalid(
                "keyboard moveresize must not carry a pointer button",
            ));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_snapshot_preserves_independent_above_and_attention_state() {
        let state = WindowFeatureSnapshot {
            above: true,
            demands_attention: true,
            ..WindowFeatureSnapshot::default()
        };
        assert!(state.enabled(WindowFeature::Above));
        assert!(state.enabled(WindowFeature::DemandsAttention));
    }

    #[test]
    fn pointer_moveresize_requires_root_coordinates() {
        let request = InteractiveMoveResize {
            window: WindowRef {
                id: 7,
                generation: 2,
            },
            direction: MoveResizeDirection::Move,
            root_position: None,
            button: 1,
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn keyboard_moveresize_rejects_pointer_button() {
        let request = InteractiveMoveResize {
            window: WindowRef {
                id: 7,
                generation: 2,
            },
            direction: MoveResizeDirection::MoveKeyboard,
            root_position: None,
            button: 1,
        };
        assert!(request.validate().is_err());
    }
}
