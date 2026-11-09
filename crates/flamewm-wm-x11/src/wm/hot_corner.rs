//! `hotCorner.*` dispatch. Pointer position feeds the pure dwell state from `flamewm-shell-core`.
//! The pointer is sampled on `XInput2` raw motion selected on the root, because core pointer motion
//! is not delivered to the root while a client window covers the corner. The only timer is the one
//! pending dwell deadline, which the event loop folds into its wait. Raw motion is selected only
//! while a corner has an action, and no polling runs while idle.

use std::time::{Duration, Instant};

use flamewm_platform::settings::HotCornerAction;
use flamewm_shell_core::hot_corner::{Corner, CornerDwell, Dwell};
use x11rb::protocol::xproto::EventMask;

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
    /// Whether the root currently selects `XI_RawMotion` (hot corners or shake-to-locate).
    pub(super) root_motion: bool,
    /// Whether hot corners are configured, so raw motion samples the pointer for dwell.
    pub(super) corners: bool,
}

impl HotCornerState {
    pub(super) fn new() -> Self {
        Self {
            dwell: CornerDwell::default(),
            epoch: Instant::now(),
            deadline: None,
            root_motion: false,
            corners: false,
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

/// Raw motion is selected while either feature needs it, and dropped when neither does.
pub(super) fn raw_motion_selected(corners: bool, shake: bool) -> bool {
    corners || shake
}

/// Root event mask. Pointer motion is not in it: hot corners read raw motion instead.
pub(super) fn root_event_mask() -> EventMask {
    EventMask::SUBSTRUCTURE_REDIRECT
        | EventMask::SUBSTRUCTURE_NOTIFY
        | EventMask::PROPERTY_CHANGE
        | EventMask::KEY_PRESS
        | EventMask::KEY_RELEASE
}

impl<C: Connection> Wm<'_, C> {
    /// Samples the pointer and updates the dwell for the corner under it. Runs on every raw
    /// motion record and on a due dwell, since a resting pointer sends no motion.
    pub(super) fn hot_corner_sample(&mut self) -> Result<(), ReplyError> {
        if !self.hot_corner.corners {
            return Ok(());
        }
        let reply = self.conn.query_pointer(self.screen().root)?.reply()?;
        let corner = if reply.same_screen {
            let fallback = self.work_area();
            let (output, _) = self.snap_zones_at(reply.root_x, reply.root_y, fallback);
            corner_at(i32::from(reply.root_x), i32::from(reply.root_y), output)
        } else {
            None
        };
        self.hot_corner_update(corner)
    }

    /// Per-turn upkeep: follow `hotCorner.*` changes and collect a due dwell.
    pub(super) fn tick_hot_corner(&mut self) -> Result<(), ReplyError> {
        let corners = live_settings::hot_corners_enabled();
        let shake = live_settings::shake_to_locate_enabled();
        if !corners && self.hot_corner.corners {
            // Turning corners off drops any pending dwell; the reset clears the selection too.
            self.hot_corner = HotCornerState::new();
        }
        self.hot_corner.corners = corners;
        let motion = raw_motion_selected(corners, shake);
        if motion != self.hot_corner.root_motion {
            self.hot_corner.root_motion = motion;
            // Without XInput there is no raw feed, and hot corners stay inert.
            if let Some(opcode) = self.raw_keys.as_ref().map(|raw| raw.opcode) {
                self.send_root_xi_select(opcode, motion)?;
            }
        }
        let due = self
            .hot_corner
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        if due {
            self.hot_corner_sample()?;
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
    fn raw_motion_is_selected_while_either_feature_needs_it() {
        assert!(raw_motion_selected(true, false), "hot corners alone");
        assert!(raw_motion_selected(false, true), "shake alone");
        assert!(raw_motion_selected(true, true), "both");
        assert!(!raw_motion_selected(false, false), "neither: deselected");
    }

    #[test]
    fn root_mask_never_selects_core_pointer_motion() {
        assert!(!root_event_mask().contains(EventMask::POINTER_MOTION));
        assert!(root_event_mask().contains(EventMask::SUBSTRUCTURE_REDIRECT));
    }

    #[test]
    fn raw_motion_record_is_recognised_by_type_and_opcode() {
        // GenericEvent header: type 35, XInput opcode, sequence, length, then evtype at byte 8.
        let mut record = [0u8; 32];
        record[0] = 35;
        record[1] = 131;
        record[8..10].copy_from_slice(&17u16.to_ne_bytes());
        assert!(super::super::raw_keys::is_raw_motion(&record, 131));
        assert!(!super::super::raw_keys::is_raw_motion(&record, 130));
        record[8..10].copy_from_slice(&13u16.to_ne_bytes());
        assert!(!super::super::raw_keys::is_raw_motion(&record, 131));
        assert!(!super::super::raw_keys::is_raw_motion(&record[..9], 131));
    }
}
