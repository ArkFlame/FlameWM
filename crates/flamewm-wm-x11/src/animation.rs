//! Window transition state machine for minimize, restore, maximize and open.
//!
//! Pure data: no X11 calls and no clock reads. Callers inject `now`, own the overlay
//! pixels, and apply the authoritative WM operation when [`Tick::Complete`] arrives.
//! Time advances through the shared [`flamewm_reactor::Timeline`], so there is one
//! timeline owner and no second animation loop.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_reactor::{Easing, Start, Timeline, TransitionId};

/// Base durations in milliseconds at 1.0x speed.
pub const MINIMIZE_MS: u32 = 260;
pub const MAXIMIZE_MS: u32 = 200;
pub const OPEN_MS: u32 = 160;

/// Capture attempts a window-open effect makes while its window still paints blank.
pub const OPEN_CAPTURE_ATTEMPTS: u8 = 3;

/// Bounds for cached minimize captures: at most this many entries, and at most this many bytes.
pub const CAPTURE_CACHE_MAX_ENTRIES: usize = 8;
pub const CAPTURE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Integer screen rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Transition kind. `Restore` is the reverse of `Minimize`; `Unmaximize` is the reverse of
/// `Maximize` (maximized rect to the restored floating rect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Minimize,
    Restore,
    Maximize,
    Unmaximize,
    Open,
}

/// Bounded cache of captures keyed by client. Evicts the oldest entry while either the entry
/// count or the byte total exceeds its cap. An entry larger than the byte cap is not kept.
#[derive(Debug)]
pub struct CaptureCache<T> {
    order: VecDeque<u32>,
    entries: HashMap<u32, (usize, T)>,
    bytes: usize,
}

impl<T> Default for CaptureCache<T> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            entries: HashMap::new(),
            bytes: 0,
        }
    }
}

impl<T> CaptureCache<T> {
    /// Inserts `value` of `size` bytes for `key`, replacing any previous entry for `key`.
    pub fn insert(&mut self, key: u32, value: T, size: usize) {
        self.remove(key);
        if size > CAPTURE_CACHE_MAX_BYTES {
            return;
        }
        self.entries.insert(key, (size, value));
        self.order.push_back(key);
        self.bytes += size;
        while self.order.len() > CAPTURE_CACHE_MAX_ENTRIES || self.bytes > CAPTURE_CACHE_MAX_BYTES {
            let Some(oldest) = self.order.front().copied() else {
                break;
            };
            self.remove(oldest);
        }
    }

    /// Removes and returns the cached value for `key`.
    pub fn take(&mut self, key: u32) -> Option<T> {
        let (size, value) = self.entries.remove(&key)?;
        self.bytes -= size;
        self.order.retain(|queued| *queued != key);
        Some(value)
    }

    /// Drops the entry for `key`, if any (for example on window destroy).
    pub fn remove(&mut self, key: u32) {
        let _ = self.take(key);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// User settings snapshot. `minimize` gates both minimize and restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub speed: u16,
    pub minimize: bool,
    pub maximize: bool,
    pub open: bool,
}

impl Settings {
    /// Defaults used until the first settings sync point publishes values.
    pub const DEFAULT: Self = Self {
        enabled: true,
        speed: 100,
        minimize: true,
        maximize: true,
        open: true,
    };
}

/// Maps `animationsEnabled`, `animationSpeed` (hundredths), and the per-effect flags.
/// Missing or mistyped keys keep the default for that key.
#[must_use]
pub fn settings_from_snapshot(snapshot: &SettingsSnapshot) -> Settings {
    let flag = |key: &str, default: bool| match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    };
    let speed = match snapshot.values.get("animationSpeed") {
        Some(SettingValue::Integer(value)) => u16::try_from(*value).unwrap_or(Settings::DEFAULT.speed),
        _ => Settings::DEFAULT.speed,
    };
    Settings {
        enabled: flag("animationsEnabled", Settings::DEFAULT.enabled),
        speed,
        minimize: flag("minimizeAnimationEnabled", Settings::DEFAULT.minimize),
        maximize: flag("maximizeAnimationEnabled", Settings::DEFAULT.maximize),
        open: flag("windowOpenAnimationEnabled", Settings::DEFAULT.open),
    }
}

static LIVE_SETTINGS: Mutex<Settings> = Mutex::new(Settings::DEFAULT);

/// Publishes the settings sync point's animation values for the WM owner thread.
pub fn set_live_settings(settings: Settings) {
    if let Ok(mut live) = LIVE_SETTINGS.lock() {
        *live = settings;
    }
}

/// Latest published animation settings.
#[must_use]
pub fn live_settings() -> Settings {
    LIVE_SETTINGS.lock().map_or(Settings::DEFAULT, |live| *live)
}

/// Result of [`Animator::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// No transition was created; the caller applies the end state now.
    Immediate,
    /// A new transition runs; the caller shows the overlay and arms the timer.
    Started,
    /// An opposite transition for the same client was reversed from its progress.
    Reversed,
    /// The same effect is already running for this client; nothing changes.
    AlreadyInFlight,
}

/// Output of [`Animator::tick`]. `Complete` is emitted exactly once per transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tick {
    Frame { client: u32, progress: f32 },
    Complete { client: u32, effect: Effect },
}

#[derive(Debug, Clone, Copy)]
struct Flight {
    client: u32,
    id: TransitionId,
    effect: Effect,
}

/// Target rectangle for a minimize or restore animation.
///
/// Uses `_NET_WM_ICON_GEOMETRY` when it is non-empty. Otherwise falls back to a
/// point on the bottom (taskbar) edge at the horizontal center of `output`.
#[must_use]
pub fn minimize_target(icon_geometry: Option<Rect>, output: Rect) -> Rect {
    match icon_geometry {
        Some(rect) if rect.width > 0 && rect.height > 0 => rect,
        _ => Rect {
            x: output.x + output.width / 2,
            y: output.y + output.height - 1,
            width: 1,
            height: 1,
        },
    }
}

/// Linear interpolation between two rectangles at `progress` in `[0, 1]`.
#[must_use]
pub fn lerp_rect(from: Rect, to: Rect, progress: f32) -> Rect {
    let p = progress.clamp(0.0, 1.0);
    let mix = |a: i32, b: i32| (a as f32 + (b - a) as f32 * p).round() as i32;
    Rect {
        x: mix(from.x, to.x),
        y: mix(from.y, to.y),
        width: mix(from.width, to.width).max(1),
        height: mix(from.height, to.height).max(1),
    }
}

/// Overlay warp parameter for a flight progress value. A mirrored overlay (restore) plays
/// target→source, so progress 0 starts at the target. Reversal keeps the overlay continuous
/// because the mirror flag never changes after the overlay is opened.
#[must_use]
pub fn overlay_t(progress: f32, mirrored: bool) -> f64 {
    let p = f64::from(progress.clamp(0.0, 1.0));
    if mirrored { 1.0 - p } else { p }
}

/// True when every pixel of an ARGB32 capture equals the first one. A window that has not
/// painted yet captures as one solid colour (its frame background), so it is not worth animating.
#[must_use]
pub fn is_blank_capture(argb: &[u8]) -> bool {
    let mut pixels = argb.chunks_exact(4);
    let Some(first) = pixels.next() else {
        return true;
    };
    pixels.all(|pixel| pixel == first)
}

/// Pending window-open effects. A managed window arms here when it is mapped; the effect
/// starts on a later Expose once the capture is non-blank. Arming never delays the map.
#[derive(Debug, Default)]
pub struct OpenGate {
    /// Remaining capture attempts per armed client.
    armed: HashMap<u32, u8>,
}

impl OpenGate {
    pub fn arm(&mut self, client: u32) {
        self.armed.insert(client, OPEN_CAPTURE_ATTEMPTS);
    }

    #[must_use]
    pub fn is_armed(&self, client: u32) -> bool {
        self.armed.contains_key(&client)
    }

    /// Records one capture attempt. Returns `true` when the capture is usable; the client is
    /// then disarmed and the caller starts the effect. A blank capture spends one attempt, and
    /// the last one disarms without starting, so a window that never paints is not retried.
    pub fn attempt(&mut self, client: u32, blank: bool) -> bool {
        let Some(left) = self.armed.get_mut(&client) else {
            return false;
        };
        if !blank {
            self.armed.remove(&client);
            return true;
        }
        *left -= 1;
        if *left == 0 {
            self.armed.remove(&client);
        }
        false
    }

    /// Drops the pending effect for `client` (input to it, unmap, or destroy).
    pub fn clear(&mut self, client: u32) {
        self.armed.remove(&client);
    }

}

/// Owns all in-flight transitions on a single [`Timeline`].
#[derive(Debug)]
pub struct Animator {
    timeline: Timeline<Effect>,
    flights: Vec<Flight>,
    settings: Settings,
    xrender_transform: bool,
    capability_logged: bool,
}

impl Animator {
    /// Creates an animator. `xrender_transform` is the XRender transform capability;
    /// without it every transition is immediate, so no fake effect is drawn.
    #[must_use]
    pub fn new(xrender_transform: bool, settings: Settings) -> Self {
        let mut timeline = Timeline::new();
        timeline.set_enabled(settings.enabled && xrender_transform);
        timeline.set_speed(settings.speed);
        Self {
            timeline,
            flights: Vec::new(),
            settings,
            xrender_transform,
            capability_logged: false,
        }
    }

    /// Replaces the settings. When transitions become disabled, every flight is drained and
    /// returned as `(client, effect)` so the caller can apply each end state exactly once.
    pub fn apply_settings(&mut self, settings: Settings) -> Vec<(u32, Effect)> {
        self.settings = settings;
        self.timeline.set_enabled(self.timeline_enabled());
        self.timeline.set_speed(settings.speed);
        if self.timeline_enabled() {
            Vec::new()
        } else {
            self.cancel_all()
        }
    }

    fn timeline_enabled(&self) -> bool {
        self.settings.enabled && self.xrender_transform
    }

    /// One-time capability line for the WM log.
    pub fn take_capability_log(&mut self) -> Option<String> {
        if self.capability_logged {
            return None;
        }
        self.capability_logged = true;
        Some(format!(
            "animation: xrender_transform={} enabled={}",
            self.xrender_transform, self.settings.enabled
        ))
    }

    fn effect_allowed(&self, effect: Effect) -> bool {
        match effect {
            Effect::Minimize | Effect::Restore => self.settings.minimize,
            Effect::Maximize | Effect::Unmaximize => self.settings.maximize,
            Effect::Open => self.settings.open,
        }
    }

    fn base_ms(effect: Effect) -> u32 {
        match effect {
            Effect::Minimize | Effect::Restore => MINIMIZE_MS,
            Effect::Maximize | Effect::Unmaximize => MAXIMIZE_MS,
            Effect::Open => OPEN_MS,
        }
    }

    fn opposite(effect: Effect) -> Option<Effect> {
        match effect {
            Effect::Minimize => Some(Effect::Restore),
            Effect::Restore => Some(Effect::Minimize),
            _ => None,
        }
    }

    /// Starts `effect` for `client`. An opposite minimize/restore flight reverses in place.
    /// A different effect while one is running returns `AlreadyInFlight`.
    pub fn start(&mut self, client: u32, effect: Effect, now: Instant) -> Begin {
        if let Some(index) = self.flights.iter().position(|f| f.client == client) {
            let current = self.flights[index].effect;
            if current == effect {
                return Begin::AlreadyInFlight;
            }
            if Self::opposite(current) == Some(effect) {
                let id = self.flights[index].id;
                self.timeline.reverse(id, now);
                self.flights[index].effect = effect;
                return Begin::Reversed;
            }
            // Any other effect while one runs is refused; the caller must `cancel` first and
            // apply that flight's end state itself.
            return Begin::AlreadyInFlight;
        }
        if !self.effect_allowed(effect) || !self.timeline_enabled() {
            return Begin::Immediate;
        }
        let id = TransitionId::from(client);
        let base = Self::base_ms(effect);
        match self.timeline.start(id, effect, now, base, Easing::Smoothstep) {
            Start::Immediate => Begin::Immediate,
            Start::Started => {
                self.flights.push(Flight { client, id, effect });
                Begin::Started
            }
        }
    }

    /// Advances all flights to `now`. Each finished flight yields exactly one `Complete`.
    pub fn tick(&mut self, now: Instant) -> Vec<Tick> {
        self.timeline.prune(now);
        let timeline = &self.timeline;
        let mut out = Vec::new();
        self.flights.retain(|flight| match timeline.progress(flight.id, now) {
            Some(progress) => {
                out.push(Tick::Frame {
                    client: flight.client,
                    progress,
                });
                true
            }
            None => {
                out.push(Tick::Complete {
                    client: flight.client,
                    effect: flight.effect,
                });
                false
            }
        });
        out
    }

    /// Removes the flight for `client` without a `Complete`. Returns the effect that was
    /// running so the caller can apply its end state immediately.
    pub fn cancel(&mut self, client: u32) -> Option<Effect> {
        let index = self.flights.iter().position(|f| f.client == client)?;
        let flight = self.flights.swap_remove(index);
        self.timeline.cancel(flight.id);
        Some(flight.effect)
    }

    /// Cancels every flight, e.g. on output change. Returns `(client, effect)` pairs.
    pub fn cancel_all(&mut self) -> Vec<(u32, Effect)> {
        let flights = std::mem::take(&mut self.flights);
        flights
            .into_iter()
            .map(|flight| {
                self.timeline.cancel(flight.id);
                (flight.client, flight.effect)
            })
            .collect()
    }

    /// Effect currently running for `client`, if any.
    #[must_use]
    pub fn running(&self, client: u32) -> Option<Effect> {
        self.flights
            .iter()
            .find(|f| f.client == client)
            .map(|f| f.effect)
    }

    /// True while any flight is active. The shared timer should run only then.
    /// True when `effect` would run as an animation now (enabled, capable, and its flag is on).
    #[must_use]
    pub fn wants(&self, effect: Effect) -> bool {
        self.timeline_enabled() && self.effect_allowed(effect)
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.flights.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const ON: Settings = Settings {
        enabled: true,
        speed: 100,
        minimize: true,
        maximize: true,
        open: true,
    };

    fn animator() -> Animator {
        Animator::new(true, ON)
    }

    fn completes(ticks: &[Tick]) -> Vec<(u32, Effect)> {
        ticks
            .iter()
            .filter_map(|t| match *t {
                Tick::Complete { client, effect } => Some((client, effect)),
                Tick::Frame { .. } => None,
            })
            .collect()
    }

    #[test]
    fn minimize_completes_once_then_is_silent() {
        let t0 = Instant::now();
        let mut a = animator();
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Started);
        let late = t0 + Duration::from_secs(5);
        assert_eq!(completes(&a.tick(late)), vec![(7, Effect::Minimize)]);
        assert!(completes(&a.tick(late)).is_empty(), "no second commit");
        assert!(!a.is_active());
    }

    #[test]
    fn rapid_repeat_of_same_effect_is_a_no_op() {
        let t0 = Instant::now();
        let mut a = animator();
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Started);
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::AlreadyInFlight);
        let done = completes(&a.tick(t0 + Duration::from_secs(5)));
        assert_eq!(done, vec![(7, Effect::Minimize)], "exactly one commit");
    }

    #[test]
    fn cancel_returns_effect_and_never_completes() {
        let t0 = Instant::now();
        let mut a = animator();
        a.start(7, Effect::Minimize, t0);
        assert_eq!(a.cancel(7), Some(Effect::Minimize));
        assert_eq!(a.cancel(7), None);
        assert!(completes(&a.tick(t0 + Duration::from_secs(5))).is_empty());
        assert!(!a.is_active());
    }

    #[test]
    fn cancel_all_drains_every_flight_once() {
        let t0 = Instant::now();
        let mut a = animator();
        a.start(1, Effect::Minimize, t0);
        a.start(2, Effect::Open, t0);
        let mut drained = a.cancel_all();
        drained.sort_by_key(|(c, _)| *c);
        assert_eq!(drained, vec![(1, Effect::Minimize), (2, Effect::Open)]);
        assert!(a.cancel_all().is_empty());
        assert!(!a.is_active());
    }

    #[test]
    fn disabled_path_creates_no_flight_and_no_timer_state() {
        let t0 = Instant::now();
        let mut a = Animator::new(true, Settings { enabled: false, ..ON });
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Immediate);
        assert!(!a.is_active());
        assert!(a.tick(t0).is_empty());
    }

    #[test]
    fn per_effect_flag_gates_only_that_effect() {
        let t0 = Instant::now();
        let mut a = Animator::new(true, Settings { maximize: false, ..ON });
        assert_eq!(a.start(7, Effect::Maximize, t0), Begin::Immediate);
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Started);
    }

    #[test]
    fn missing_capability_is_immediate_and_logged_once() {
        let t0 = Instant::now();
        let mut a = Animator::new(false, ON);
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Immediate);
        assert!(!a.is_active());
        let first = a.take_capability_log().expect("first log line");
        assert!(first.contains("xrender_transform=false"));
        assert!(a.take_capability_log().is_none());
    }

    #[test]
    fn restore_during_minimize_reverses_without_double_completion() {
        let t0 = Instant::now();
        let mut a = animator();
        a.start(7, Effect::Minimize, t0);
        let mid = t0 + Duration::from_millis(100);
        assert_eq!(a.start(7, Effect::Restore, mid), Begin::Reversed);
        let end = t0 + Duration::from_secs(5);
        assert_eq!(completes(&a.tick(end)), vec![(7, Effect::Restore)]);
        assert!(completes(&a.tick(end)).is_empty());
    }

    #[test]
    fn reversed_frames_return_toward_start() {
        let t0 = Instant::now();
        let mut a = animator();
        a.start(7, Effect::Minimize, t0);
        let forward_end = t0 + Duration::from_millis(200);
        let p_forward = match a.tick(forward_end)[0] {
            Tick::Frame { progress, .. } => progress,
            other => panic!("expected frame, got {other:?}"),
        };
        a.start(7, Effect::Restore, forward_end);
        let p_back = match a.tick(forward_end + Duration::from_millis(1))[0] {
            Tick::Frame { progress, .. } => progress,
            other => panic!("expected frame, got {other:?}"),
        };
        assert!(p_back <= p_forward, "reverse must not jump forward");
    }

    #[test]
    fn minimize_target_prefers_icon_geometry() {
        let out = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let icon = Rect { x: 40, y: 1050, width: 32, height: 24 };
        assert_eq!(minimize_target(Some(icon), out), icon);
    }

    #[test]
    fn minimize_target_fallback_is_taskbar_center_not_origin() {
        let out = Rect { x: 1920, y: 0, width: 1920, height: 1080 };
        let empty = Rect { x: 0, y: 0, width: 0, height: 0 };
        let t = minimize_target(Some(empty), out);
        assert_eq!(t, minimize_target(None, out));
        assert_eq!(t.x, 1920 + 960);
        assert_eq!(t.y, 1079);
        assert_ne!((t.x, t.y), (0, 0));
    }

    #[test]
    fn settings_map_keys_and_keep_defaults_for_missing_values() {
        use flamewm_api::settings::{SettingValue, SettingsSnapshot};
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert("animationsEnabled".into(), SettingValue::Boolean(false));
        snapshot.values.insert("animationSpeed".into(), SettingValue::Integer(50));
        snapshot.values.insert("maximizeAnimationEnabled".into(), SettingValue::Boolean(false));
        let mapped = settings_from_snapshot(&snapshot);
        assert!(!mapped.enabled);
        assert_eq!(mapped.speed, 50);
        assert!(!mapped.maximize);
        assert!(mapped.minimize && mapped.open, "missing keys keep the default");
        assert_eq!(settings_from_snapshot(&SettingsSnapshot::new(1)), Settings::DEFAULT);
    }

    #[test]
    fn restore_overlay_starts_at_target_and_ends_at_window() {
        assert_eq!(overlay_t(0.0, true), 1.0, "restore begins at the icon");
        assert_eq!(overlay_t(1.0, true), 0.0, "restore ends at the window");
        assert_eq!(overlay_t(0.25, false), 0.25, "minimize is not mirrored");
    }

    #[test]
    fn running_reports_the_flight_effect_until_it_completes() {
        let t0 = Instant::now();
        let mut a = animator();
        assert_eq!(a.running(7), None);
        a.start(7, Effect::Minimize, t0);
        assert_eq!(a.running(7), Some(Effect::Minimize));
        a.tick(t0 + Duration::from_secs(5));
        assert_eq!(a.running(7), None);
    }

    #[test]
    fn unmaximize_is_gated_by_maximize_flag_and_completes_once() {
        let t0 = Instant::now();
        let mut off = Animator::new(true, Settings { maximize: false, ..ON });
        assert_eq!(off.start(7, Effect::Unmaximize, t0), Begin::Immediate);
        let mut a = animator();
        assert_eq!(a.start(7, Effect::Unmaximize, t0), Begin::Started);
        assert_eq!(a.running(7), Some(Effect::Unmaximize));
        let late = t0 + Duration::from_secs(5);
        assert_eq!(completes(&a.tick(late)), vec![(7, Effect::Unmaximize)]);
        assert!(completes(&a.tick(late)).is_empty(), "no second commit");
    }

    #[test]
    fn maximize_and_unmaximize_do_not_reverse_each_other() {
        let t0 = Instant::now();
        let mut a = animator();
        assert_eq!(a.start(7, Effect::Maximize, t0), Begin::Started);
        assert_eq!(a.start(7, Effect::Unmaximize, t0), Begin::AlreadyInFlight);
        assert_eq!(a.running(7), Some(Effect::Maximize));
    }

    #[test]
    fn capture_cache_evicts_oldest_past_entry_cap() {
        let mut cache: CaptureCache<u32> = CaptureCache::default();
        for client in 0..(CAPTURE_CACHE_MAX_ENTRIES as u32 + 2) {
            cache.insert(client, client, 1024);
        }
        assert_eq!(cache.len(), CAPTURE_CACHE_MAX_ENTRIES);
        assert_eq!(cache.take(0), None, "oldest evicted");
        assert_eq!(cache.take(1), None, "second oldest evicted");
        assert_eq!(cache.take(2), Some(2));
        assert_eq!(cache.bytes(), (CAPTURE_CACHE_MAX_ENTRIES - 1) * 1024);
    }

    #[test]
    fn capture_cache_evicts_oldest_past_byte_cap() {
        let mut cache: CaptureCache<&str> = CaptureCache::default();
        let half = CAPTURE_CACHE_MAX_BYTES / 2;
        cache.insert(1, "a", half);
        cache.insert(2, "b", half);
        cache.insert(3, "c", half);
        assert!(cache.bytes() <= CAPTURE_CACHE_MAX_BYTES);
        assert_eq!(cache.take(1), None);
        assert_eq!(cache.take(3), Some("c"));
    }

    #[test]
    fn capture_cache_rejects_oversize_and_drops_on_remove() {
        let mut cache: CaptureCache<u8> = CaptureCache::default();
        cache.insert(1, 1, CAPTURE_CACHE_MAX_BYTES + 1);
        assert!(cache.is_empty(), "single oversize capture is never kept");
        cache.insert(2, 2, 100);
        cache.remove(2);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn capture_cache_reinsert_replaces_without_double_counting() {
        let mut cache: CaptureCache<u8> = CaptureCache::default();
        cache.insert(5, 1, 300);
        cache.insert(5, 2, 200);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 200);
        assert_eq!(cache.take(5), Some(2));
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn blank_capture_is_one_solid_colour() {
        assert!(is_blank_capture(&[]));
        assert!(is_blank_capture(&[1, 2, 3, 255, 1, 2, 3, 255]));
        assert!(!is_blank_capture(&[1, 2, 3, 255, 9, 2, 3, 255]));
    }

    #[test]
    fn open_gate_arming_starts_nothing_until_a_non_blank_expose() {
        let t0 = Instant::now();
        let mut a = animator();
        let mut gate = OpenGate::default();
        gate.arm(7);
        assert!(gate.is_armed(7));
        assert_eq!(a.running(7), None, "arming must not start a flight or delay the map");
        // First Expose paints blank: spend an attempt, do not start.
        assert!(!gate.attempt(7, true));
        assert!(gate.is_armed(7));
        // Next Expose is paintable: start exactly once and disarm.
        assert!(gate.attempt(7, false));
        assert!(!gate.is_armed(7));
        assert_eq!(a.start(7, Effect::Open, t0), Begin::Started);
    }

    #[test]
    fn open_gate_gives_up_after_blank_attempts_and_clear_cancels_pending() {
        let mut gate = OpenGate::default();
        gate.arm(1);
        for _ in 0..OPEN_CAPTURE_ATTEMPTS {
            assert!(!gate.attempt(1, true));
        }
        assert!(!gate.is_armed(1), "a never-painting window is not retried forever");
        assert!(!gate.attempt(1, false), "no pending effect after exhaustion");
        gate.arm(2);
        gate.clear(2);
        assert!(!gate.attempt(2, false), "input or unmap before the expose cancels the open");
    }

    #[test]
    fn open_wants_requires_enabled_capable_and_flag() {
        let on = Animator::new(true, Settings::DEFAULT);
        assert!(on.wants(Effect::Open));
        let flag_off = Animator::new(true, Settings { open: false, ..Settings::DEFAULT });
        assert!(!flag_off.wants(Effect::Open));
        let no_cap = Animator::new(false, Settings::DEFAULT);
        assert!(!no_cap.wants(Effect::Open));
        let disabled = Animator::new(true, Settings { enabled: false, ..Settings::DEFAULT });
        assert!(!disabled.wants(Effect::Open));
    }

    #[test]
    fn lerp_rect_endpoints_are_exact() {
        let a = Rect { x: 10, y: 20, width: 300, height: 200 };
        let b = Rect { x: 500, y: 900, width: 32, height: 24 };
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
    }
}
