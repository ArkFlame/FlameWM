//! `hotCorner.*` dispatch. Root pointer motion feeds the pure dwell state from `flamewm-shell-core`.
//! The only timer is the one pending dwell deadline, which the event loop folds into its wait.
//! No polling runs while idle, and root motion is selected only while a corner has an action.

use std::time::{Duration, Instant};

use flamewm_platform::settings::HotCornerAction;
use flamewm_shell_core::hot_corner::{Corner, CornerDwell, Dwell};
use x11rb::protocol::xproto::{ConnectionExt as _, EventMask};

use super::*;
use crate::live_settings;

/// A pointer within this many pixels of an output corner counts as being in that corner.
const CORNER_PX: i32 = 2;

pub(super) struct HotCornerState {
    dwell: CornerDwell,
    /// Origin of the dwell clock; `CornerDwell` takes milliseconds since this instant.
    epoch: Instant,
    /// Wake-up for a pending dwell. `None` while no dwell is pending.
    pub(super) deadline: Option<Instant>,
    /// Whether the root window currently selects `POINTER_MOTION`.
    root_motion: bool,
}

impl HotCornerState {
    pub(super) fn new() -> Self {
        Self {
            dwell: CornerDwell::default(),
            epoch: Instant::now(),
            deadline: None,
            root_motion: false,
        }
    }
}

/// Corner containing the point, if it is within `CORNER_PX` of two edges of `output`.
fn corner_at(x: i32, y: i32, output: Rect) -> Option<Corner> {
    let right = output.x + i32::try_from(output.width).unwrap_or(i32::MAX);
    let bottom = output.y + i32::try_from(output.height).unwrap_or(i32::MAX);
    if x < output.x || x >= right || y < output.y || y >= bottom {
        return None;
    }
    let left = x < output.x + CORNER_PX;
    let right_edge = x >= right - CORNER_PX;
    let top = y < output.y + CORNER_PX;
    let bottom_edge = y >= bottom - CORNER_PX;
    match (left, right_edge, top, bottom_edge) {
        (true, _, true, _) => Some(Corner::TopLeft),
        (_, true, true, _) => Some(Corner::TopRight),
        (true, _, _, true) => Some(Corner::BottomLeft),
        (_, true, _, true) => Some(Corner::BottomRight),
        _ => None,
    }
}

/// Event-loop wait capped at the pending dwell deadline. `None` waits for an event.
pub(super) fn hot_corner_wait(
    deadline: Option<Instant>,
    now: Instant,
    wait: Option<Duration>,
) -> Option<Duration> {
    let Some(deadline) = deadline else {
        return wait;
    };
    let remaining = deadline.saturating_duration_since(now);
    Some(wait.map_or(remaining, |other| other.min(remaining)))
}

/// Root event mask: the base mask, plus pointer motion while a hot corner is configured.
pub(super) fn root_event_mask(pointer_motion: bool) -> EventMask {
    let base = EventMask::SUBSTRUCTURE_REDIRECT
        | EventMask::SUBSTRUCTURE_NOTIFY
        | EventMask::PROPERTY_CHANGE
        | EventMask::KEY_PRESS
        | EventMask::KEY_RELEASE;
    if pointer_motion {
        base | EventMask::POINTER_MOTION
    } else {
        base
    }
}

impl<C: Connection> Wm<'_, C> {
    /// Root pointer motion: update the dwell for the corner under the pointer.
    pub(super) fn hot_corner_motion(&mut self, x: i16, y: i16) -> Result<(), ReplyError> {
        let fallback = self.work_area();
        let (output, _) = self.snap_zones_at(x, y, fallback);
        let corner = corner_at(i32::from(x), i32::from(y), output);
        self.hot_corner_update(corner)
    }

    /// Per-turn upkeep: follow `hotCorner.*` changes and collect a due dwell. A due dwell
    /// re-reads the pointer, since a resting pointer sends no motion.
    pub(super) fn tick_hot_corner(&mut self) -> Result<(), ReplyError> {
        let enabled = live_settings::hot_corners_enabled();
        if enabled != self.hot_corner.root_motion {
            self.hot_corner.root_motion = enabled;
            if !enabled {
                self.hot_corner = HotCornerState::new();
            }
            let root = self.screen().root;
            self.conn.change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(root_event_mask(enabled)),
            )?;
        }
        let due = self
            .hot_corner
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        if due {
            let reply = self.conn.query_pointer(self.screen().root)?.reply()?;
            let corner = if reply.same_screen {
                let fallback = self.work_area();
                let (output, _) = self.snap_zones_at(reply.root_x, reply.root_y, fallback);
                corner_at(i32::from(reply.root_x), i32::from(reply.root_y), output)
            } else {
                None
            };
            self.hot_corner_update(corner)?;
        }
        Ok(())
    }

    fn hot_corner_update(&mut self, corner: Option<Corner>) -> Result<(), ReplyError> {
        let now_ms = u64::try_from(self.hot_corner.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        match self.hot_corner.dwell.update(corner, now_ms) {
            Dwell::Idle => self.hot_corner.deadline = None,
            Dwell::Pending { deadline_ms, .. } => {
                self.hot_corner.deadline =
                    Some(self.hot_corner.epoch + Duration::from_millis(deadline_ms));
            }
            Dwell::Fire(corner) => {
                self.hot_corner.deadline = None;
                self.run_hot_corner(corner)?;
            }
        }
        Ok(())
    }

    fn run_hot_corner(&mut self, corner: Corner) -> Result<(), ReplyError> {
        match live_settings::hot_corner_action(corner) {
            // Overview has no X11 backend yet; the setting is accepted and ignored.
            HotCornerAction::None | HotCornerAction::Overview => {}
            HotCornerAction::ShowDesktop => self.toggle_show_desktop()?,
            HotCornerAction::Lock => {
                if let Err(error) = flamewm_x11_desktop::start_lock_screen() {
                    eprintln!("flamewm: hot corner lock failed: {error:?}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Rect {
        Rect::new(0, 0, 1000, 800)
    }

    #[test]
    fn corner_zones_are_the_outer_pixels_of_each_corner() {
        assert_eq!(corner_at(0, 0, output()), Some(Corner::TopLeft));
        assert_eq!(corner_at(999, 0, output()), Some(Corner::TopRight));
        assert_eq!(corner_at(0, 799, output()), Some(Corner::BottomLeft));
        assert_eq!(corner_at(999, 799, output()), Some(Corner::BottomRight));
        assert_eq!(
            corner_at(CORNER_PX - 1, CORNER_PX - 1, output()),
            Some(Corner::TopLeft)
        );
        assert_eq!(corner_at(CORNER_PX, 0, output()), None);
        assert_eq!(corner_at(500, 0, output()), None);
        assert_eq!(corner_at(0, 400, output()), None);
    }

    #[test]
    fn points_outside_the_output_are_not_corners() {
        assert_eq!(corner_at(-1, 0, output()), None);
        assert_eq!(corner_at(1000, 0, output()), None);
        assert_eq!(corner_at(0, 800, output()), None);
    }

    #[test]
    fn wait_is_capped_by_a_pending_dwell_and_untouched_when_idle() {
        let now = Instant::now();
        let ten = Duration::from_millis(10);
        let hundred = Duration::from_millis(100);
        assert_eq!(hot_corner_wait(None, now, Some(hundred)), Some(hundred));
        assert_eq!(hot_corner_wait(None, now, None), None);
        assert_eq!(
            hot_corner_wait(Some(now + hundred), now, None),
            Some(hundred)
        );
        assert_eq!(
            hot_corner_wait(Some(now + hundred), now, Some(ten)),
            Some(ten)
        );
        assert_eq!(
            hot_corner_wait(Some(now + ten), now, Some(hundred)),
            Some(ten)
        );
    }

    #[test]
    fn root_mask_adds_pointer_motion_only_when_requested() {
        assert!(root_event_mask(true).contains(EventMask::POINTER_MOTION));
        assert!(!root_event_mask(false).contains(EventMask::POINTER_MOTION));
        assert!(root_event_mask(false).contains(EventMask::SUBSTRUCTURE_REDIRECT));
    }
}
