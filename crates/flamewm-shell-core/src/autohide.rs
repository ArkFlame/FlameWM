//! Dock auto-hide timing. Pure state: no X11, surfaces, or settings access.
//!
//! The dock is held open while the pointer is over it or over its reveal strip, while a
//! menu or drag keeps it open, and for `HIDE_DELAY` after the last hold ends. Otherwise it
//! slides out over `SLIDE`. A fullscreen window on the dock output hides it even when
//! auto-hide is off.

use std::time::{Duration, Instant};

/// Grace period after the last hold before the dock slides away.
pub const HIDE_DELAY: Duration = Duration::from_millis(400);
/// Full slide duration between shown and hidden.
pub const SLIDE: Duration = Duration::from_millis(200);
/// Frame interval while a slide is running.
const FRAME: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoHide {
    auto_hide: bool,
    fullscreen: bool,
    pointer_in: bool,
    pinned_open: bool,
    /// Set when the last hold ends; the dock may not hide before this instant.
    hide_at: Option<Instant>,
    /// 0.0 fully shown, 1.0 fully hidden. Moves toward the target by time.
    progress: f32,
    last: Instant,
}

impl AutoHide {
    /// Starts shown. With auto-hide on, the dock slides away after the grace period.
    #[must_use]
    pub fn new(auto_hide: bool, now: Instant) -> Self {
        Self {
            auto_hide,
            fullscreen: false,
            pointer_in: false,
            pinned_open: false,
            hide_at: auto_hide.then(|| now + HIDE_DELAY),
            progress: 0.0,
            last: now,
        }
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

    /// Pointer over the bar or its reveal strip.
    pub fn set_pointer(&mut self, inside: bool, now: Instant) {
        self.set_hold(inside, Hold::Pointer, now);
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
        self.pointer_in || self.pinned_open
    }

    fn set_hold(&mut self, on: bool, hold: Hold, now: Instant) {
        let was_held = self.held();
        match hold {
            Hold::Pointer => self.pointer_in = on,
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
        let stride = elapsed.as_secs_f32() / SLIDE.as_secs_f32();
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
        if (self.progress - target).abs() > f32::EPSILON {
            return Some(now + FRAME);
        }
        self.hide_at.filter(|at| *at > now)
    }
}

#[derive(Clone, Copy)]
enum Hold {
    Pointer,
    Pinned,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(model: &mut AutoHide, mut now: Instant, until: Duration) -> Instant {
        let end = now + until;
        while now < end {
            now += Duration::from_millis(16);
            model.step(now);
        }
        now
    }

    #[test]
    fn auto_hide_starts_hidden_after_grace() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        assert!(!m.is_hidden());
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden());
        assert!(m.wants_hidden(t));
    }

    #[test]
    fn pointer_shows_immediately_and_leaving_waits_for_delay() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        m.set_pointer(true, t);
        let t = settle(&mut m, t, SLIDE);
        assert_eq!(m.offset(), 0.0);
        m.set_pointer(false, t);
        let t = settle(&mut m, t, HIDE_DELAY - Duration::from_millis(50));
        assert!(!m.wants_hidden(t), "still inside the grace period");
        assert_eq!(m.offset(), 0.0);
        settle(&mut m, t, Duration::from_millis(100) + SLIDE);
        assert!(m.is_hidden());
    }

    #[test]
    fn pinned_menu_or_drag_keeps_dock_open() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(true, t0);
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
        let mut m = AutoHide::new(false, t0);
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
        let mut m = AutoHide::new(false, t0);
        m.set_pointer(true, t0);
        m.set_pointer(false, t0);
        let t = settle(&mut m, t0, HIDE_DELAY * 3);
        assert_eq!(m.offset(), 0.0);
        assert!(m.deadline(t).is_none());
    }

    #[test]
    fn turning_auto_hide_on_starts_grace_then_hides() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(false, t0);
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
        let mut m = AutoHide::new(true, t0);
        let t = settle(&mut m, t0, HIDE_DELAY + SLIDE);
        assert!(m.is_hidden(), "precondition: the dock slid away");
        // dock.autoHide=false applied live: the dock must slide back to shown, not stay parked.
        m.set_auto_hide(false, t);
        let t = settle(&mut m, t, SLIDE);
        assert!(!m.is_hidden());
        assert_eq!(m.offset(), 0.0);
        assert!(!m.wants_hidden(t));
    }

    #[test]
    fn offset_is_monotonic_and_bounded() {
        let t0 = Instant::now();
        let mut m = AutoHide::new(false, t0);
        m.set_fullscreen(true);
        let mut last = 0.0;
        let mut now = t0;
        for _ in 0..30 {
            now += FRAME;
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
        let mut m = AutoHide::new(true, t0);
        assert_eq!(m.deadline(t0), Some(t0 + HIDE_DELAY));
        m.set_pointer(true, t0);
        assert!(
            m.deadline(t0).is_none(),
            "shown and held: nothing to wake for"
        );
        m.set_pointer(false, t0);
        m.set_pointer(true, t0);
        assert!(m.deadline(t0).is_none());
    }
}
