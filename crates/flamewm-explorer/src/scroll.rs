//! Pure timing and geometry for the explorer list: the sub-row scroll glide, resize coalescing,
//! and scrollbar thumb maths. No UI or clock reads happen here; callers pass `Instant`s in.
use std::time::{Duration, Instant};

/// Length of one glide. Every wheel step restarts it from the displayed position, so a burst settles this long after its last step.
pub const GLIDE_DURATION: Duration = Duration::from_millis(70);
/// Interval at which the window size is applied while a resize drag is in progress (trailing edge always applies).
pub const RESIZE_INTERVAL: Duration = Duration::from_millis(250);
/// Smallest scrollbar thumb, in pixels.
pub const MIN_THUMB: f32 = 24.0;

/// Largest visual lead, in rows, between the drawn rows and their resting origin.
pub fn max_lead(rows: usize) -> f32 {
    rows.max(1) as f32 * 2.0
}

/// Sub-row offset of the drawn list. Rows are drawn at `index - origin + offset`, so the offset is zero at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowGlide {
    from: f32,
    start: Option<Instant>,
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl RowGlide {
    /// Offset in rows at `now`. Exactly zero once the glide has run its course, so it never drifts.
    pub fn offset(&self, now: Instant) -> f32 {
        let Some(start) = self.start else { return 0.0 };
        let t = now.saturating_duration_since(start).as_secs_f32() / GLIDE_DURATION.as_secs_f32();
        if t >= 1.0 {
            0.0
        } else {
            self.from * (1.0 - ease_out_cubic(t))
        }
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.start
            .is_some_and(|start| now.saturating_duration_since(start) < GLIDE_DURATION)
    }

    /// The window origin moved by `step` rows. The rows stay where they are on screen, then glide to rest.
    /// The starting offset is clamped, so repeated steps cannot pile up. A jump of more than `rows` snaps.
    pub fn origin_moved(&mut self, step: isize, now: Instant, rows: usize) {
        if step.unsigned_abs() > rows {
            *self = Self::default();
            return;
        }
        let lead = max_lead(rows);
        let from = (self.offset(now) + step as f32).clamp(-lead, lead);
        *self = Self {
            from,
            start: Some(now),
        };
    }
}

/// Holds resize sizes so that at most one relayout happens per `RESIZE_INTERVAL`.
#[derive(Debug, Default)]
pub struct ResizeGate {
    applied: Option<Instant>,
    pending: Option<(u32, u32)>,
}

/// What the caller must do with a size offered to the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resize {
    /// Apply and project now (leading edge).
    Apply,
    /// Held; the caller must schedule `flush` after this delay.
    Defer(Duration),
}

impl ResizeGate {
    pub fn offer(&mut self, size: (u32, u32), now: Instant) -> Resize {
        match self.applied {
            Some(at) if now.saturating_duration_since(at) < RESIZE_INTERVAL => {
                self.pending = Some(size);
                Resize::Defer(RESIZE_INTERVAL - now.saturating_duration_since(at))
            }
            _ => {
                self.applied = Some(now);
                self.pending = None;
                Resize::Apply
            }
        }
    }

    /// Trailing edge: returns the most recent held size, if any, and marks it applied.
    pub fn flush(&mut self, now: Instant) -> Option<(u32, u32)> {
        let size = self.pending.take()?;
        self.applied = Some(now);
        Some(size)
    }
}

/// Scrollbar thumb length for `total` items with `rows` visible. `None` when everything fits.
pub fn thumb_length(track: f32, total: usize, rows: usize) -> Option<f32> {
    (rows > 0 && total > rows)
        .then(|| (track * rows as f32 / total as f32).clamp(MIN_THUMB.min(track), track))
}

/// Thumb `(top, length)` within a track of `track` pixels, for the window origin `origin`.
pub fn thumb_geometry(track: f32, total: usize, rows: usize, origin: usize) -> Option<(f32, f32)> {
    let length = thumb_length(track, total, rows)?;
    let max_origin = (total - rows) as f32;
    let top = (track - length) * (origin.min(total - rows) as f32 / max_origin);
    Some((top, length))
}

/// Origin after dragging the thumb by `dy` pixels from `start_origin`. Clamped to the list.
pub fn drag_origin(track: f32, total: usize, rows: usize, start_origin: usize, dy: f32) -> usize {
    let Some(length) = thumb_length(track, total, rows) else {
        return 0;
    };
    let travel = track - length;
    if travel <= 0.0 {
        return start_origin.min(total - rows);
    }
    let rows_per_px = (total - rows) as f32 / travel;
    let target = start_origin as f32 + dy * rows_per_px;
    target.round().clamp(0.0, (total - rows) as f32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_of_notches_settles_within_200ms_of_last_event() {
        let t0 = Instant::now();
        let rows = 10;
        let mut glide = RowGlide::default();
        let mut now = t0;
        let mut origin: isize = 0;
        // 100 wheel notches, 3 rows each, 2 ms apart: the origin moves 300 rows in one burst.
        for _ in 0..100 {
            origin += 3;
            glide.origin_moved(3, now, rows);
            assert!(
                glide.offset(now).abs() <= max_lead(rows) + 1e-4,
                "lead must stay bounded"
            );
            now += Duration::from_millis(2);
        }
        assert_eq!(origin, 300);
        let last_event = now - Duration::from_millis(2);
        assert!(glide.is_animating(last_event));
        let settled = last_event + Duration::from_millis(200);
        assert_eq!(glide.offset(settled), 0.0, "settled exactly, not drifting");
        assert!(!glide.is_animating(settled));
    }

    #[test]
    fn offset_starts_at_the_visual_position_and_decays_monotonically() {
        let t0 = Instant::now();
        let mut glide = RowGlide::default();
        glide.origin_moved(3, t0, 10);
        let first = glide.offset(t0);
        assert_eq!(first, 3.0, "rows keep their on-screen position at the step");
        let mut previous = first;
        for ms in (10..=130).step_by(10) {
            let value = glide.offset(t0 + Duration::from_millis(ms));
            assert!(value <= previous && value >= 0.0);
            previous = value;
        }
        assert_eq!(glide.offset(t0 + GLIDE_DURATION), 0.0);
    }

    #[test]
    fn jumps_larger_than_a_window_snap_without_animation() {
        let t0 = Instant::now();
        let mut glide = RowGlide::default();
        glide.origin_moved(25, t0, 10);
        assert_eq!(glide.offset(t0), 0.0);
        assert!(!glide.is_animating(t0));
    }

    #[test]
    fn resize_applies_leading_edge_and_holds_the_final_size_for_the_trailing_edge() {
        let t0 = Instant::now();
        let mut gate = ResizeGate::default();
        assert_eq!(gate.offer((800, 600), t0), Resize::Apply);
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(
            gate.offer((820, 600), ms(50)),
            Resize::Defer(Duration::from_millis(200))
        );
        assert_eq!(
            gate.offer((900, 700), ms(100)),
            Resize::Defer(Duration::from_millis(150))
        );
        assert_eq!(
            gate.flush(ms(250)),
            Some((900, 700)),
            "trailing edge applies the final size"
        );
        assert_eq!(gate.flush(ms(260)), None, "nothing left to apply");
        assert_eq!(
            gate.offer((1000, 700), ms(300)),
            Resize::Defer(Duration::from_millis(200))
        );
        assert_eq!(
            gate.offer((1000, 700), ms(600)),
            Resize::Apply,
            "quiet interval applies at once"
        );
    }

    #[test]
    fn scrollbar_thumb_reflects_viewport_and_position() {
        assert_eq!(
            thumb_length(300.0, 10, 10),
            None,
            "everything fits: no scrollbar"
        );
        assert_eq!(thumb_length(300.0, 30, 10), Some(100.0));
        assert_eq!(
            thumb_length(300.0, 3000, 10),
            Some(MIN_THUMB),
            "very long lists keep a grabbable thumb"
        );
        assert_eq!(thumb_geometry(300.0, 30, 10, 0), Some((0.0, 100.0)));
        assert_eq!(thumb_geometry(300.0, 30, 10, 20), Some((200.0, 100.0)));
    }

    #[test]
    fn dragging_the_thumb_maps_pixels_to_rows_and_clamps() {
        // 30 items, 10 visible, 100 px thumb on a 300 px track: 200 px of travel for 20 rows.
        assert_eq!(drag_origin(300.0, 30, 10, 0, 100.0), 10);
        assert_eq!(drag_origin(300.0, 30, 10, 10, -50.0), 5);
        assert_eq!(
            drag_origin(300.0, 30, 10, 0, 10_000.0),
            20,
            "clamped to the last window"
        );
        assert_eq!(
            drag_origin(300.0, 30, 10, 5, -10_000.0),
            0,
            "clamped to the first window"
        );
        assert_eq!(
            drag_origin(300.0, 5, 10, 0, 50.0),
            0,
            "no scrolling when everything fits"
        );
    }
}
