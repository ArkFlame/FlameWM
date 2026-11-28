//! Pure per-second raster cap for the XP-style resize outline, with a trailing edge.
//!
//! Precedent: `.vendor/xfwm4-master/src/poswin.c` `poswinSetPosition` recomputes the label from
//! the newest geometry and only calls `gtk_widget_queue_draw` (line 139); GTK coalesces the
//! queued draws into one raster per frame. We port that "queue latest, draw once per cadence,
//! always draw the last" behaviour as a pure typed gate: no X calls and no hidden clock reads.
//! The live caller injects `now` and supplies the geometry payload.

use std::time::{Duration, Instant};

/// Maximum resize-outline rasters admitted in any one-second window.
pub(super) const OUTLINE_RASTER_CAP_PER_SEC: u32 = 60;

/// Minimum spacing between rasters, rounded *up* to whole milliseconds so a 1 kHz motion stream
/// can never exceed [`OUTLINE_RASTER_CAP_PER_SEC`] rasters in one second (including the trailing
/// flush).
#[allow(clippy::cast_lossless)]
const OUTLINE_RASTER_INTERVAL: Duration =
    Duration::from_millis(1_000_u64.div_ceil(OUTLINE_RASTER_CAP_PER_SEC as u64));

/// What the caller does with one motion's outline geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RasterDecision {
    /// Present this geometry now; the gate consumed one raster slot.
    Now,
    /// Hold the newest geometry and present it no earlier than this instant.
    Wait(Instant),
}

/// Typed facade over resize-outline raster timing. `T` is the geometry payload (the live caller
/// supplies `RootRect`); the gate never inspects it, it only guarantees the newest queued value
/// wins.
#[derive(Debug)]
pub(super) struct OutlineRasterGate<T> {
    last_raster: Option<Instant>,
    /// Deadline and newest geometry queued while paced (trailing edge).
    trailing: Option<(Instant, T)>,
}

impl<T> Default for OutlineRasterGate<T> {
    fn default() -> Self {
        Self {
            last_raster: None,
            trailing: None,
        }
    }
}

impl<T> OutlineRasterGate<T> {
    /// Record one outline motion. `Now` means this motion may raster; `Wait` means the geometry
    /// is retained as the trailing edge until the deadline.
    #[must_use]
    pub(super) fn note(&mut self, now: Instant, geometry: T) -> RasterDecision {
        match self.last_raster {
            Some(last) if now.saturating_duration_since(last) < OUTLINE_RASTER_INTERVAL => {
                let due = last + OUTLINE_RASTER_INTERVAL;
                self.trailing = Some((due, geometry)); // newest motion wins
                RasterDecision::Wait(due)
            }
            _ => {
                self.last_raster = Some(now);
                self.trailing = None;
                RasterDecision::Now
            }
        }
    }

    /// Take the trailing geometry once its deadline elapsed. The flush is itself a raster and
    /// consumes the next slot.
    #[must_use]
    pub(super) fn take_due(&mut self, now: Instant) -> Option<T> {
        if self.trailing.as_ref().is_some_and(|(due, _)| now >= *due) {
            self.last_raster = Some(now);
            self.trailing.take().map(|(_, geometry)| geometry)
        } else {
            None
        }
    }

    /// Caps a reactor wait so the trailing raster lands on its deadline.
    #[must_use]
    pub(super) fn cap_wait(&self, now: Instant, wait: Option<Duration>) -> Option<Duration> {
        let remaining = self.trailing.as_ref().map_or(Duration::ZERO, |(due, _)| {
            due.saturating_duration_since(now)
        });
        Some(wait.map_or(remaining, |wait| wait.min(remaining)))
    }

    /// Drop pending state when the drag ends or is aborted.
    pub(super) fn clear(&mut self) {
        self.trailing = None;
        self.last_raster = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> OutlineRasterGate<u32> {
        OutlineRasterGate::default()
    }

    #[test]
    fn first_motion_rasters_now_and_a_burst_waits_until_the_deadline() {
        let start = Instant::now();
        let mut g = gate();
        assert_eq!(g.note(start, 1), RasterDecision::Now);
        assert_eq!(
            g.note(start + Duration::from_millis(5), 2),
            RasterDecision::Wait(start + OUTLINE_RASTER_INTERVAL)
        );
        assert_eq!(
            g.note(start + OUTLINE_RASTER_INTERVAL, 3),
            RasterDecision::Now
        );
    }

    #[test]
    fn one_second_of_1khz_motions_never_exceeds_the_cap_and_ends_on_final_geometry() {
        let start = Instant::now();
        let mut g = gate();
        let mut rasters = 0u32;
        let mut pending = None;
        for ms in 0..1_000u64 {
            let now = start + Duration::from_millis(ms);
            let geometry = ms as u32;
            match g.note(now, geometry) {
                RasterDecision::Now => {
                    rasters += 1;
                    pending = None;
                }
                RasterDecision::Wait(due) => pending = Some((due, geometry)),
            }
            if let Some((due, _)) = pending
                && now >= due
            {
                assert!(g.take_due(now).is_some());
                rasters += 1;
                pending = None;
            }
        }
        assert!(
            rasters <= OUTLINE_RASTER_CAP_PER_SEC,
            "1 kHz resize emitted {rasters} outline rasters, cap is {OUTLINE_RASTER_CAP_PER_SEC}"
        );
        assert!(rasters > 0, "the first motion must raster immediately");
        // The final motion is held back; drain the trailing edge at its real deadline.
        let (due, geometry) = pending.expect("the last motion stays queued");
        assert_eq!(geometry, 999, "the newest geometry wins");
        rasters += 1;
        assert!(
            rasters <= OUTLINE_RASTER_CAP_PER_SEC,
            "trailing flush pushed rasters to {rasters}"
        );
        assert_eq!(
            g.take_due(due),
            Some(geometry),
            "final geometry is drawn once"
        );
    }

    #[test]
    fn trailing_edge_carries_the_latest_geometry_and_lands_on_its_deadline() {
        let start = Instant::now();
        let mut g = gate();
        assert_eq!(g.note(start, 10), RasterDecision::Now);
        let deadline = start + OUTLINE_RASTER_INTERVAL;
        assert_eq!(
            g.note(start + Duration::from_millis(5), 11),
            RasterDecision::Wait(deadline)
        );
        assert_eq!(
            g.note(start + Duration::from_millis(10), 12),
            RasterDecision::Wait(deadline)
        );
        let left = OUTLINE_RASTER_INTERVAL - Duration::from_millis(10);
        assert_eq!(
            g.cap_wait(start + Duration::from_millis(10), None),
            Some(left)
        );
        assert_eq!(g.take_due(start + Duration::from_millis(10)), None);
        assert_eq!(g.take_due(deadline), Some(12), "latest geometry wins");
        assert!(g.take_due(deadline).is_none(), "a flush is one-shot");
    }

    #[test]
    fn clear_drops_pending_state_for_cancel_or_release() {
        let start = Instant::now();
        let mut g = gate();
        assert_eq!(g.note(start, 1), RasterDecision::Now);
        assert_eq!(
            g.note(start + Duration::from_millis(1), 2),
            RasterDecision::Wait(start + OUTLINE_RASTER_INTERVAL)
        );
        g.clear();
        assert_eq!(g.take_due(start + Duration::from_secs(1)), None);
        assert_eq!(
            g.note(start + Duration::from_secs(1), 3),
            RasterDecision::Now
        );
    }
}
