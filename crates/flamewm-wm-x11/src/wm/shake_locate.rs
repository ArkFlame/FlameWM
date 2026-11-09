//! Shake-to-locate glue. Raw motion feeds the pure detector and enlarge/restore state from
//! `flamewm-platform`. A shake enlarges the cursor through the WM's cursor-theme owners, and the
//! nominal size comes back when the window ends. The only timer is the pending restore deadline,
//! folded into the event-loop wait like the hot-corner dwell.

use std::time::{Duration, Instant};

use flamewm_platform::shake::{ShakeEvent, ShakeLocate, enlarged_cursor_size};

use super::*;
use crate::live_settings;

pub(super) struct ShakeState {
    machine: ShakeLocate,
    /// Origin of the machine clock, in milliseconds.
    epoch: Instant,
}

impl ShakeState {
    pub(super) fn new() -> Self {
        Self {
            machine: ShakeLocate::new(),
            epoch: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Wake-up for the pending restore. `None` while the nominal size is in effect.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.machine
            .deadline_ms()
            .map(|ms| self.epoch + Duration::from_millis(ms))
    }
}

/// Applies one cursor size to every WM-owned cursor. Failures are logged, never fatal.
fn apply_cursor_size(size: u32) {
    // The renderer registry reloads only when the theme or size changed, so this is idempotent.
    let theme = std::env::var("XCURSOR_THEME")
        .ok()
        .filter(|theme| !theme.is_empty())
        .unwrap_or_else(|| "default".to_owned());
    for error in flamewm_render_x11::apply_cursor_appearance(&theme, Some(size)) {
        eprintln!("flamewm: shake-to-locate cursor size failed: {error}");
    }
}

impl<C: Connection> Wm<'_, C> {
    /// Feeds one raw motion record to the detector. Enlarges the cursor on the completing sample.
    pub(super) fn shake_feed(&mut self, bytes: &[u8], opcode: u8) {
        let Some((time, dx)) = super::raw_keys::raw_motion_dx(bytes, opcode) else {
            return;
        };
        let now_ms = self.shake_locate.now_ms();
        if self.shake_locate.machine.feed_motion(time, dx, now_ms) == ShakeEvent::Enlarged {
            apply_cursor_size(enlarged_cursor_size(live_settings::cursor_size()));
        }
    }

    /// Per-turn upkeep: restores the nominal size when the window ends or the setting went off.
    pub(super) fn tick_shake_locate(&mut self) {
        let enabled = live_settings::shake_to_locate_enabled();
        let now_ms = self.shake_locate.now_ms();
        if self.shake_locate.machine.restore_due(now_ms, enabled) {
            apply_cursor_size(live_settings::cursor_size());
        }
    }
}
