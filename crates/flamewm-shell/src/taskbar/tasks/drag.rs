//! Pure drag gesture for taskbar task slots. A press arms the gesture; motion
//! beyond `DRAG_THRESHOLD_PX` turns it into a drag; release either reports a
//! plain click (the normal activate/minimize path) or a reorder.

/// Logical pixels a press must travel before it becomes a drag.
pub const DRAG_THRESHOLD_PX: f32 = 6.0;
/// Logical height of the taskbar strip (`.taskbar` / `.task-slot` height).
pub const TASK_STRIP_HEIGHT_PX: f32 = 44.0;
/// Logical pitch of one task slot (`.task-slot` width).
pub const SLOT_PITCH_PX: f32 = 42.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskDrag {
    source: usize,
    origin_x: f32,
    dragging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDragRelease {
    /// Short press: the caller runs the normal slot click.
    Click,
    /// Drag ended at `to` (final insertion index).
    Reorder { from: usize, to: usize },
    /// Drag ended at its own slot, or was cancelled: nothing to do.
    Noop,
}

impl TaskDrag {
    #[must_use]
    pub const fn press(source: usize, x: f32) -> Self {
        Self {
            source,
            origin_x: x,
            dragging: false,
        }
    }

    /// Returns true once the gesture is a drag.
    pub fn motion(&mut self, x: f32) -> bool {
        if !self.dragging && (x - self.origin_x).abs() > DRAG_THRESHOLD_PX {
            self.dragging = true;
        }
        self.dragging
    }

    /// Final insertion index for a pointer at `x`, clamped to the slot range.
    #[must_use]
    pub fn target(&self, x: f32, slot_count: usize) -> usize {
        let last = slot_count.saturating_sub(1);
        let shift = ((x - self.origin_x) / SLOT_PITCH_PX).round();
        (self.source as f32 + shift).clamp(0.0, last as f32) as usize
    }

    /// Release that lands off the task strip (outside `0..TASK_STRIP_HEIGHT_PX` vertically)
    /// cancels an active drag. `y` is in panel-surface coordinates.
    #[must_use]
    pub fn release_on_strip(self, x: f32, y: f32, slot_count: usize) -> TaskDragRelease {
        let on_strip = (0.0..TASK_STRIP_HEIGHT_PX).contains(&y);
        if self.dragging && !on_strip {
            return TaskDragRelease::Noop;
        }
        self.release(x, slot_count)
    }

    #[must_use]
    pub fn release(self, x: f32, slot_count: usize) -> TaskDragRelease {
        if !self.dragging {
            return TaskDragRelease::Click;
        }
        let to = self.target(x, slot_count);
        if to == self.source || slot_count == 0 {
            TaskDragRelease::Noop
        } else {
            TaskDragRelease::Reorder {
                from: self.source,
                to,
            }
        }
    }
}

/// Left edge of the 2 px insertion marker placed before slot `index`, relative
/// to the task-slot strip.
#[must_use]
pub fn insertion_marker_left(index: usize) -> f32 {
    index as f32 * SLOT_PITCH_PX - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_marker_sits_on_the_slot_boundary() {
        assert_eq!(insertion_marker_left(0), -1.0);
        assert_eq!(insertion_marker_left(2), 83.0);
        let drag = TaskDrag::press(0, 0.0);
        assert_eq!(drag.target(2.0 * SLOT_PITCH_PX, 3), 2);
    }

    #[test]
    fn short_press_without_movement_is_a_click() {
        let mut drag = TaskDrag::press(1, 100.0);
        assert!(!drag.motion(104.0));
        assert_eq!(drag.release(104.0, 3), TaskDragRelease::Click);
    }

    #[test]
    fn exactly_threshold_is_still_a_click() {
        let mut drag = TaskDrag::press(0, 100.0);
        assert!(!drag.motion(106.0));
        assert_eq!(drag.release(106.0, 3), TaskDragRelease::Click);
    }

    #[test]
    fn movement_past_threshold_is_a_drag_and_never_a_click() {
        let mut drag = TaskDrag::press(0, 100.0);
        assert!(drag.motion(106.5));
        // Returning inside the threshold keeps the drag: no accidental activate.
        assert!(drag.motion(100.0));
        assert_eq!(drag.release(100.0, 3), TaskDragRelease::Noop);
    }

    #[test]
    fn drag_right_by_two_slots_reorders_to_index_two() {
        let mut drag = TaskDrag::press(0, 20.0);
        assert!(drag.motion(60.0));
        assert_eq!(
            drag.release(20.0 + 2.0 * SLOT_PITCH_PX, 3),
            TaskDragRelease::Reorder { from: 0, to: 2 }
        );
    }

    #[test]
    fn drag_left_reorders_and_clamps_to_bounds() {
        let mut drag = TaskDrag::press(2, 200.0);
        assert!(drag.motion(150.0));
        assert_eq!(
            drag.release(-500.0, 3),
            TaskDragRelease::Reorder { from: 2, to: 0 }
        );
        let mut drag = TaskDrag::press(0, 0.0);
        assert!(drag.motion(50.0));
        assert_eq!(
            drag.release(9_000.0, 3),
            TaskDragRelease::Reorder { from: 0, to: 2 }
        );
    }

    #[test]
    fn release_off_the_strip_cancels_the_drag() {
        let mut drag = TaskDrag::press(0, 10.0);
        assert!(drag.motion(200.0));
        // Released far to the right but off the strip: nothing happens.
        assert_eq!(drag.release_on_strip(200.0, -200.0, 3), TaskDragRelease::Noop);
        // Same release inside the strip reorders as before.
        let mut drag = TaskDrag::press(0, 10.0);
        assert!(drag.motion(200.0));
        assert_eq!(
            drag.release_on_strip(200.0, 20.0, 3),
            TaskDragRelease::Reorder { from: 0, to: 2 }
        );
    }

    #[test]
    fn negative_control_small_delta_does_not_reorder() {
        // A plausible wrong fix would reorder on any motion; one slot pitch
        // is needed before the insertion index changes.
        let mut drag = TaskDrag::press(0, 10.0);
        assert!(drag.motion(40.0));
        assert_eq!(drag.release(30.0, 3), TaskDragRelease::Noop);
    }
}
