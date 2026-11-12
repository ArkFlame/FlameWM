//! Alt+Tab thumbnail provider: never blocks the event loop.
//!
//! `request` only reads the cache and schedules a capture; it never touches X. The event loop calls
//! `CaptureBudget::begin_turn` once per turn and then `ThumbProvider::pump`, which runs at most
//! `per_turn` captures. Pure over the `ThumbSource` seam: the X11 implementation lives in
//! `switcher_thumb_x11.rs`, tests use a fake.

use std::collections::{HashSet, VecDeque};

use super::switcher_thumbs::{ThumbCache, ThumbKey, capture_to_thumb};

/// Cached thumbnail: `(w, h, pixels)`.
pub(crate) type Thumb = (u32, u32, Vec<u8>);

/// Window state the skip rules read. Only a mapped, viewable, non-iconic window is captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowState {
    pub(crate) mapped: bool,
    pub(crate) viewable: bool,
    pub(crate) iconic: bool,
}

impl WindowState {
    pub(crate) fn capturable(self) -> bool {
        self.mapped && self.viewable && !self.iconic
    }
}

/// Raw 32-bit pixels read from a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Capture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: usize,
    pub(crate) data: Vec<u8>,
}

/// Seam over the X server: window state and pixel readback.
pub(crate) trait ThumbSource {
    /// `None` when the window is gone.
    fn state(&mut self, window: u32) -> Option<WindowState>;
    /// `None` when the read fails.
    fn capture(&mut self, window: u32) -> Option<Capture>;
}

/// Captures allowed per event-loop turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureBudget {
    per_turn: u32,
    left: u32,
}

impl CaptureBudget {
    pub(crate) fn new(per_turn: u32) -> Self {
        Self {
            per_turn,
            left: per_turn,
        }
    }

    /// Refills the budget; call once at the start of each event-loop turn.
    pub(crate) fn begin_turn(&mut self) {
        self.left = self.per_turn;
    }

    /// Spends one capture from this turn's budget; `false` when exhausted.
    pub(crate) fn take(&mut self) -> bool {
        if self.left == 0 {
            return false;
        }
        self.left -= 1;
        true
    }
}

#[derive(Debug, Default)]
pub(crate) struct ThumbProvider {
    cache: ThumbCache<Thumb>,
    /// FIFO of keys waiting for a capture; at most one entry per window.
    pending: VecDeque<ThumbKey>,
    /// Windows damaged since their last successful capture: never served from cache.
    dirty: HashSet<u32>,
}

impl ThumbProvider {
    /// Returns the cached thumbnail for `key`, or `None` and schedules a capture. Repeated
    /// requests for one window keep a single pending entry (the newest key).
    pub(crate) fn request(&mut self, key: ThumbKey) -> Option<&Thumb> {
        let hit = !self.dirty.contains(&key.window) && self.cache.get(key).is_some();
        if hit {
            return self.cache.get(key);
        }
        self.pending.retain(|k| k.window != key.window);
        self.pending.push_back(key);
        None
    }

    /// Marks `window` damaged: drops its thumbnails and blocks serving them until recaptured.
    pub(crate) fn damage(&mut self, window: u32) {
        self.cache.remove_window(window);
        self.dirty.insert(window);
    }

    /// Starts a switcher session. Cached frames of windows `hidden` (minimised or on another
    /// workspace) stay served as their last captured frame. Every other cached window is
    /// invalidated, since it may have repainted while the switcher was closed.
    pub(crate) fn begin_session(&mut self, hidden: impl Fn(u32) -> bool) {
        let visible: Vec<u32> = self.cache.windows().filter(|&w| !hidden(w)).collect();
        for window in visible {
            self.damage(window);
        }
    }

    /// Ends a switcher session: queued captures are dropped, cached frames stay (LRU-bounded).
    pub(crate) fn end_session(&mut self) {
        self.pending.clear();
    }

    /// Window closed or destroyed: drops everything kept for it.
    pub(crate) fn forget(&mut self, window: u32) {
        self.cache.remove_window(window);
        self.pending.retain(|k| k.window != window);
        self.dirty.remove(&window);
    }

    /// Runs up to the budget's remaining captures in FIFO order. Windows that are not capturable
    /// are dropped from the queue without spending budget. A capture whose size differs from its
    /// key is discarded (the window resized; the newer key is requested again by the UI). Returns
    /// the number of thumbnails stored.
    pub(crate) fn pump<S: ThumbSource>(
        &mut self,
        source: &mut S,
        budget: &mut CaptureBudget,
    ) -> usize {
        let mut stored = 0;
        while let Some(&key) = self.pending.front() {
            if !source
                .state(key.window)
                .is_some_and(WindowState::capturable)
            {
                self.pending.pop_front();
                continue;
            }
            if !budget.take() {
                break;
            }
            self.pending.pop_front();
            let Some(cap) = source.capture(key.window) else {
                continue;
            };
            if (cap.width, cap.height) != (key.src_w, key.src_h) {
                continue;
            }
            if let Some(thumb) = capture_to_thumb(&cap.data, cap.width, cap.height, cap.stride) {
                // One size per window: a resize replaces the old thumbnail instead of adding one.
                self.cache.remove_window(key.window);
                self.cache.insert(key, thumb);
                self.dirty.remove(&key.window);
                stored += 1;
            }
        }
        stored
    }

    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const LIVE: WindowState = WindowState {
        mapped: true,
        viewable: true,
        iconic: false,
    };

    fn key(window: u32) -> ThumbKey {
        ThumbKey {
            window,
            src_w: 640,
            src_h: 400,
        }
    }

    #[derive(Default)]
    struct Fake {
        states: HashMap<u32, WindowState>,
        sizes: HashMap<u32, (u32, u32)>,
        captures: Vec<u32>,
    }

    impl Fake {
        fn live(windows: &[u32]) -> Self {
            let mut fake = Self::default();
            for &w in windows {
                fake.states.insert(w, LIVE);
                fake.sizes.insert(w, (640, 400));
            }
            fake
        }
    }

    impl ThumbSource for Fake {
        fn state(&mut self, window: u32) -> Option<WindowState> {
            self.states.get(&window).copied()
        }

        fn capture(&mut self, window: u32) -> Option<Capture> {
            self.captures.push(window);
            let (w, h) = *self.sizes.get(&window)?;
            Some(Capture {
                width: w,
                height: h,
                stride: w as usize * 4,
                data: vec![0x40; w as usize * h as usize * 4],
            })
        }
    }

    fn turn(provider: &mut ThumbProvider, source: &mut Fake, budget: &mut CaptureBudget) -> usize {
        budget.begin_turn();
        provider.pump(source, budget)
    }

    #[test]
    fn request_never_captures_and_schedules_once() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        assert!(p.request(key(1)).is_none());
        assert!(p.request(key(1)).is_none());
        assert!(p.request(key(1)).is_none());
        assert!(s.captures.is_empty(), "request must not touch the source");
        assert_eq!(p.pending_len(), 1, "no double scheduling");
        let mut b = CaptureBudget::new(4);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert_eq!(s.captures, vec![1], "one capture for three requests");
        assert!(p.request(key(1)).is_some());
    }

    #[test]
    fn budget_caps_captures_per_turn() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1, 2, 3]);
        for w in 1..=3 {
            p.request(key(w));
        }
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 0);
        assert_eq!(s.captures, vec![1, 2, 3]);
    }

    #[test]
    fn budget_without_begin_turn_stays_spent() {
        let mut b = CaptureBudget::new(2);
        assert!(b.take() && b.take());
        assert!(!b.take());
        b.begin_turn();
        assert!(b.take());
    }

    #[test]
    fn skips_unmapped_unviewable_and_iconic_without_spending_budget() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1, 2, 3, 4]);
        s.states.insert(
            1,
            WindowState {
                mapped: false,
                ..LIVE
            },
        );
        s.states.insert(
            2,
            WindowState {
                viewable: false,
                ..LIVE
            },
        );
        s.states.insert(
            3,
            WindowState {
                iconic: true,
                ..LIVE
            },
        );
        for w in 1..=4 {
            p.request(key(w));
        }
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert_eq!(s.captures, vec![4], "skipped windows never reach capture");
        assert_eq!(p.pending_len(), 0);
    }

    #[test]
    fn vanished_window_is_skipped() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::default();
        p.request(key(9));
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 0);
        assert!(s.captures.is_empty());
        assert_eq!(p.pending_len(), 0);
    }

    #[test]
    fn minimised_window_serves_its_last_frame_across_sessions() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        p.end_session();
        s.states.insert(
            1,
            WindowState {
                iconic: true,
                ..LIVE
            },
        );
        p.begin_session(|w| w == 1);
        assert!(
            p.request(key(1)).is_some(),
            "hidden window keeps its last frame"
        );
        assert_eq!(p.pending_len(), 0, "hidden window is not scheduled");
    }

    #[test]
    fn visible_window_is_recaptured_on_a_new_session() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        turn(&mut p, &mut s, &mut b);
        p.end_session();
        p.begin_session(|_| false);
        assert!(
            p.request(key(1)).is_none(),
            "visible window is not served stale"
        );
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        assert!(p.request(key(1)).is_some());
    }

    #[test]
    fn damage_invalidates_and_blocks_stale_serve_until_recaptured() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        turn(&mut p, &mut s, &mut b);
        assert!(p.request(key(1)).is_some());

        p.damage(1);
        assert!(
            p.request(key(1)).is_none(),
            "dirty window must not serve stale pixels"
        );
        turn(&mut p, &mut s, &mut b);
        assert_eq!(s.captures, vec![1, 1]);
        assert!(p.request(key(1)).is_some(), "clean again after recapture");
    }

    #[test]
    fn dirty_window_stays_dirty_when_capture_fails() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        s.sizes.remove(&1); // capture returns None
        p.damage(1);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 0);
        assert!(p.request(key(1)).is_none());
    }

    #[test]
    fn size_change_is_a_new_key_and_replaces_the_old_thumb() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        turn(&mut p, &mut s, &mut b);
        let resized = ThumbKey {
            src_w: 1280,
            src_h: 800,
            ..key(1)
        };
        assert!(
            p.request(resized).is_none(),
            "old-size thumb must not serve"
        );
        s.sizes.insert(1, (1280, 800));
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
        let (w, h, _) = p.request(resized).expect("new size cached");
        assert_eq!((*w, *h), (160, 100));
        assert!(p.request(key(1)).is_none(), "old size evicted");
    }

    #[test]
    fn stale_key_capture_is_discarded() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        s.sizes.insert(1, (1280, 800));
        p.request(key(1)); // key says 640x400, window is 1280x800 now
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 0);
        assert!(p.request(key(1)).is_none());
    }

    #[test]
    fn forget_drops_cache_pending_and_dirty() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1, 2]);
        p.request(key(1));
        let mut b = CaptureBudget::new(1);
        turn(&mut p, &mut s, &mut b);
        p.request(key(2));
        p.damage(1);
        p.forget(1);
        p.forget(2);
        assert_eq!(p.pending_len(), 0);
        assert!(p.request(key(1)).is_none());
    }

    #[test]
    fn request_for_new_key_replaces_pending_entry_for_window() {
        let mut p = ThumbProvider::default();
        let mut s = Fake::live(&[1]);
        p.request(key(1));
        p.request(ThumbKey {
            src_w: 1280,
            src_h: 800,
            ..key(1)
        });
        assert_eq!(p.pending_len(), 1);
        s.sizes.insert(1, (1280, 800));
        let mut b = CaptureBudget::new(1);
        assert_eq!(turn(&mut p, &mut s, &mut b), 1);
    }
}
