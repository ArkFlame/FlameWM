//! Dock auto-hide timing. Pure state: no X11, surfaces, or settings access.
//!
//! The dock is held open while the pointer is over the dock or its reveal strip, while a
//! menu or drag keeps it open, and for `HIDE_DELAY` after the last hold ends. The pointer
//! holds are tracked per surface: a Leave from one surface never clears the other's Enter.
//! Show and hide are instant unless `SLIDE_ENABLED` (animations are opt-in, default off).
//! A fullscreen window on the dock output hides it even when auto-hide is off.

use std::time::{Duration, Instant};

/// Grace period after the last hold before the dock hides. Also covers the gap between the
/// reveal strip and the dock bar, which neither surface reports as inside.
pub const HIDE_DELAY: Duration = Duration::from_millis(600);
/// Full slide duration between shown and hidden, used only when `SLIDE_ENABLED`.
pub const SLIDE: Duration = Duration::from_millis(200);
/// Opt-in switch for the slide. Off: show and hide snap to the target at once.
pub const SLIDE_ENABLED: bool = false;
/// Frame interval while a slide is running.
const FRAME: Duration = Duration::from_millis(16);

/// Which dock surface reports the pointer: the bar itself or its reveal strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerSurface {
    Dock,
    Reveal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoHide {
    auto_hide: bool,
    fullscreen: bool,
    pointer_dock: bool,
    pointer_reveal: bool,
    pinned_open: bool,
    /// Set when the last hold ends; the dock may not hide before this instant.
    hide_at: Option<Instant>,
    /// 0.0 fully shown, 1.0 fully hidden. Moves toward the target by time.
    progress: f32,
    last: Instant,
    /// True when the slide is animated; false snaps `progress` to the target on each step.
    animate: bool,
    /// `motion_scale` in hundredths: the slide lasts `SLIDE * 100 / scale`.
    scale: u16,
}

impl AutoHide {
    /// Starts shown. With auto-hide on, the dock hides after the grace period.
    #[must_use]
    pub fn new(auto_hide: bool, now: Instant) -> Self {
        Self {
            auto_hide,
            fullscreen: false,
            pointer_dock: false,
            pointer_reveal: false,
            pinned_open: false,
            hide_at: auto_hide.then(|| now + HIDE_DELAY),
            progress: 0.0,
            last: now,
            animate: SLIDE_ENABLED,
            scale: 100,
        }
    }

    /// Applies `flamewm_api::motion::motion_scale`: 0 (reducedMotion) snaps the dock, otherwise
    /// the slide (when enabled) runs at `scale` hundredths of `SLIDE`'s speed.
    pub fn set_motion(&mut self, scale: u16) {
        self.animate = SLIDE_ENABLED && scale != 0;
        self.scale = scale;
    }

    /// Applies the `dock.autoHide` setting. Turning it on starts the grace period.
    pub fn set_auto_hide(&mut self, auto_hide: bool, now: Instant) {
        if auto_hide && !self.auto_hide && !self.held() {
            self.hide_at = Some(now + HIDE_DELAY);
        }
        if !auto_hide {
            self.hide_at = None;
        }
        self.auto_hide = auto_hide;
    }

    /// Pointer inside one dock surface. The surfaces are held independently, so the order of
    /// their Enter and Leave events does not matter.
    pub fn set_pointer(&mut self, surface: PointerSurface, inside: bool, now: Instant) {
        let hold = match surface {
            PointerSurface::Dock => Hold::Dock,
            PointerSurface::Reveal => Hold::Reveal,
        };
        self.set_hold(inside, hold, now);
    }

    /// A menu or an active drag keeps the dock open.
    pub fn set_pinned_open(&mut self, open: bool, now: Instant) {
        self.set_hold(open, Hold::Pinned, now);
    }

    /// A fullscreen window is on the dock output.
    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
    }

    fn held(&self) -> bool {
        self.pointer_dock || self.pointer_reveal || self.pinned_open
    }

    fn set_hold(&mut self, on: bool, hold: Hold, now: Instant) {
        let was_held = self.held();
        match hold {
            Hold::Dock => self.pointer_dock = on,
            Hold::Reveal => self.pointer_reveal = on,
            Hold::Pinned => self.pinned_open = on,
        }
        if self.held() {
            self.hide_at = None;
        } else if was_held {
            self.hide_at = Some(now + HIDE_DELAY);
        }
    }

    /// Whether the dock should be hidden at `now`.
    #[must_use]
    pub fn wants_hidden(&self, now: Instant) -> bool {
        if self.held() || self.hide_at.is_some_and(|at| now < at) {
            return false;
        }
        self.auto_hide || self.fullscreen
    }

    /// Advances the slide toward the target at `now`.
    pub fn step(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last);
        self.last = now;
        let target = if self.wants_hidden(now) { 1.0 } else { 0.0 };
        if !self.animate {
            self.progress = target;
            return;
        }
        let stride = elapsed.as_secs_f32() * f32::from(self.scale) / (100.0 * SLIDE.as_secs_f32());
        self.progress = if self.progress < target {
            (self.progress + stride).min(target)
        } else {
            (self.progress - stride).max(target)
        };
    }

    /// Eased slide position: 0.0 shown, 1.0 hidden.
    #[must_use]
    pub fn offset(&self) -> f32 {
        let p = self.progress.clamp(0.0, 1.0);
        p * p * (3.0 - 2.0 * p)
    }

    /// True once the dock is fully hidden. The caller then unmaps it and maps the reveal strip.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.progress >= 1.0
    }

    /// Next instant the shell must tick: a frame while sliding, or the hide deadline.
    #[must_use]
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        let target = if self.wants_hidden(now) { 1.0 } else { 0.0 };
        if self.animate && (self.progress - target).abs() > f32::EPSILON {
            return Some(now + FRAME);
        }
        self.hide_at.filter(|at| *at > now)
    }
}

#[derive(Clone, Copy)]
enum Hold {
    Dock,
    Reveal,
    Pinned,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_STEP: Duration = Duration::from_millis(16);

    fn settle(model: &mut AutoHide, mut now: Instant, until: Duration) -> Instant {
        let end = now + until;
        while now < end {
            now += FRAME_STEP;
            model.step(now);
        }
        now
    }

    fn animated(auto_hide: bool, now: Instant) -> AutoHide {
        AutoHide {
            animate: true,
            ..AutoHide::new(auto_hide, now)
        }
    }

    #[test]
    fn reduced_motion_hides_at_the_deadline_without_a_slide() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        m.set_motion(0);
        m.step(t0 + HIDE_DELAY + Duration::from_millis(1));
        assert!(m.is_hidden());
    }

    #[test]
    fn slide_speed_follows_motion_scale() {
        let t0 = Instant::now();
        let at = t0 + HIDE_DELAY + Duration::from_millis(50);
        for (scale, expected) in [(100_u16, 0.25_f32), (50, 0.125)] {
            let mut m = AutoHide::new(true, t0);
            m.set_motion(scale);
            m.animate = true;
            m.last = t0 + HIDE_DELAY;
            m.step(at);
            assert!(
                (m.progress - expected).abs() < 1e-3,
                "scale {scale}: {}",
                m.progress
            );
        }
    }

    #[test]
    fn default_show_and_hide_are_instant() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        let t = t0 + HIDE_DELAY;
        m.step(t);
        assert!(m.is_hidden(), "hidden in one step, no slide");
        assert_eq!(m.offset(), 1.0);
        m.set_pointer(PointerSurface::Reveal, true, t);
        m.step(t);
        assert_eq!(m.offset(), 0.0, "shown in one step");
        assert!(m.deadline(t).is_none(), "no frame ticks without a slide");
    }

    #[test]
    fn auto_hide_starts_hidden_after_grace() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        assert!(!m.is_hidden());
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden());
        assert!(m.wants_hidden(t));
    }

    #[test]
    fn pointer_shows_immediately_and_leaving_waits_for_delay() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        m.set_pointer(PointerSurface::Reveal, true, t);
        let t = settle(&mut m, t, SLIDE);
        assert_eq!(m.offset(), 0.0);
        m.set_pointer(PointerSurface::Reveal, false, t);
        let t = settle(&mut m, t, HIDE_DELAY - Duration::from_millis(50));
        assert!(!m.wants_hidden(t), "still inside the grace period");
        assert_eq!(m.offset(), 0.0);
        settle(&mut m, t, Duration::from_millis(100) + SLIDE);
        assert!(m.is_hidden());
    }

    #[test]
    fn hide_delay_is_600_ms_after_last_hold() {
        assert_eq!(HIDE_DELAY, Duration::from_millis(600));
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        m.set_pointer(PointerSurface::Dock, true, t0);
        let leave = t0 + Duration::from_millis(1000);
        m.set_pointer(PointerSurface::Dock, false, leave);
        assert!(!m.wants_hidden(leave + Duration::from_millis(599)));
        assert!(m.wants_hidden(leave + HIDE_DELAY));
    }

    #[test]
    fn gap_between_reveal_strip_and_dock_is_covered_by_grace() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        m.set_pointer(PointerSurface::Reveal, true, t0);
        // Pointer leaves the strip, crosses the gap for 300 ms, then enters the dock.
        let leave = t0 + Duration::from_millis(500);
        m.set_pointer(PointerSurface::Reveal, false, leave);
        let enter = leave + Duration::from_millis(300);
        m.set_pointer(PointerSurface::Dock, true, enter);
        assert!(!m.wants_hidden(enter + Duration::from_secs(10)));
        assert!(m.deadline(enter + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn dock_enter_before_reveal_leave_keeps_dock_held() {
        // Surface events can arrive out of order across the two surfaces.
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        m.set_pointer(PointerSurface::Reveal, true, t0);
        m.set_pointer(PointerSurface::Dock, true, t0);
        m.set_pointer(PointerSurface::Reveal, false, t0);
        assert!(!m.wants_hidden(t0 + HIDE_DELAY * 4), "dock still holds it");
        m.set_pointer(PointerSurface::Dock, false, t0);
        assert!(m.wants_hidden(t0 + HIDE_DELAY));
    }

    #[test]
    fn pinned_menu_or_drag_keeps_dock_open() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        m.set_pinned_open(true, t0);
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert_eq!(m.offset(), 0.0);
        m.set_pinned_open(false, t);
        settle(&mut m, t, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden());
    }

    #[test]
    fn fullscreen_hides_without_auto_hide_and_restores_on_exit() {
        let t0 = Instant::now();
        let mut m = animated(false, t0);
        assert_eq!(m.offset(), 0.0);
        m.set_fullscreen(true);
        let t = settle(&mut m, t0, SLIDE);
        assert!(m.is_hidden());
        m.set_fullscreen(false);
        settle(&mut m, t, SLIDE);
        assert_eq!(m.offset(), 0.0);
    }

    #[test]
    fn disabled_auto_hide_never_hides_without_fullscreen() {
        let t0 = Instant::now();
        let mut m = animated(false, t0);
        m.set_pointer(PointerSurface::Dock, true, t0);
        m.set_pointer(PointerSurface::Dock, false, t0);
        let t = settle(&mut m, t0, HIDE_DELAY * 3);
        assert_eq!(m.offset(), 0.0);
        assert!(m.deadline(t).is_none());
    }

    #[test]
    fn turning_auto_hide_on_starts_grace_then_hides() {
        let t0 = Instant::now();
        let mut m = animated(false, t0);
        m.set_auto_hide(true, t0);
        assert!(!m.wants_hidden(t0));
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden());
        m.set_auto_hide(false, t);
        settle(&mut m, t, SLIDE);
        assert_eq!(m.offset(), 0.0);
    }

    #[test]
    fn turning_auto_hide_off_while_hidden_brings_the_dock_back() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden(), "precondition: the dock slid away");
        m.set_auto_hide(false, t);
        assert_eq!(m.deadline(t), Some(t + FRAME));
        let t = settle(&mut m, t, SLIDE);
        assert!(!m.is_hidden());
        assert_eq!(m.offset(), 0.0);
        assert!(!m.wants_hidden(t));
        assert!(
            m.deadline(t).is_none(),
            "settled and shown: no further wake"
        );
    }

    #[test]
    fn offset_is_monotonic_and_bounded() {
        let t0 = Instant::now();
        let mut m = animated(false, t0);
        m.set_fullscreen(true);
        let mut last = 0.0;
        let mut now = t0;
        for _ in 0..30 {
            now += FRAME_STEP;
            m.step(now);
            let o = m.offset();
            assert!((0.0..=1.0).contains(&o));
            assert!(o >= last);
            last = o;
        }
    }

    #[test]
    fn deadline_is_a_frame_while_sliding_and_hide_time_when_idle() {
        let t0 = Instant::now();
        let mut m = animated(true, t0);
        assert_eq!(m.deadline(t0), Some(t0 + HIDE_DELAY));
        m.set_pointer(PointerSurface::Dock, true, t0);
        assert!(
            m.deadline(t0).is_none(),
            "shown and held: nothing to wake for"
        );
        m.set_pointer(PointerSurface::Dock, false, t0);
        m.set_pointer(PointerSurface::Dock, true, t0);
        assert!(m.deadline(t0).is_none());
    }
}
