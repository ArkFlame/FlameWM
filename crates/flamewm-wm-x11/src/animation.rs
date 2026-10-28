//! Window transition state machine for minimize, restore, maximize and open.
//!
//! Pure data: no X11 calls and no clock reads. Callers inject `now`, own the overlay
//! pixels, and apply the authoritative WM operation when [`Tick::Complete`] arrives.
//! Time advances through the shared [`flamewm_reactor::Timeline`], so there is one
//! timeline owner and no second animation loop.
//!
//! The WM event loop owns the frame timer: it wakes every [`flamewm_reactor::timeline::FRAME_INTERVAL`]
//! while a transition is active and samples the timeline from there. The reactor no longer
//! carries a self-arming driver, so nothing in this module registers a calloop source.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_reactor::{Easing, Start, Timeline, TransitionId};

/// Base durations in milliseconds at 1.0x speed.
pub const MINIMIZE_MS: u32 = 260;
pub const MAXIMIZE_MS: u32 = 110;
pub const OPEN_MS: u32 = 160;
pub const CLOSE_MS: u32 = 160;

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
    /// Visual afterimage of a window the client was asked to close. The close request is never
    /// delayed by it, and the overlay outlives the window's unmanage until its timeline ends.
    Close,
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

    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Visual style of one effect. `None` means no overlay: the end state is applied immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    None,
    Scale,
    Fade,
    MagicLamp,
}

/// User settings snapshot. `minimize` gates both minimize and restore; `maximize` gates both
/// maximize and unmaximize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub speed: u16,
    pub minimize: Style,
    pub maximize: Style,
    pub open: Style,
    pub close: Style,
}

impl Settings {
    /// Defaults used until the first settings sync point publishes values.
    pub const DEFAULT: Self = Self {
        enabled: true,
        speed: 100,
        minimize: Style::None,
        maximize: Style::None,
        open: Style::None,
        close: Style::None,
    };
}

/// Style for `effect`. Only the style of the effect's own group is consulted.
#[must_use]
pub fn effect_style(settings: Settings, effect: Effect) -> Style {
    match effect {
        Effect::Minimize | Effect::Restore => settings.minimize,
        Effect::Maximize | Effect::Unmaximize => settings.maximize,
        Effect::Open => settings.open,
        Effect::Close => settings.close,
    }
}

/// True when an overlay must survive its window's unmanage. Only a running close keeps its
/// afterimage: the client is gone but the fade-out still has to finish on screen.
#[must_use]
pub fn survives_unmanage(running: Option<Effect>) -> bool {
    running == Some(Effect::Close)
}

/// Parses one per-effect style key. Disabled (`enabled_key` false) or `none` gives `None`.
/// Unknown text or a style not offered for that effect keeps `default`.
fn style_from_snapshot(
    snapshot: &SettingsSnapshot,
    enabled_key: &str,
    style_key: &str,
    default: Style,
    allowed: &[(&str, Style)],
) -> Style {
    if matches!(
        snapshot.values.get(enabled_key),
        Some(SettingValue::Boolean(false))
    ) {
        return Style::None;
    }
    match snapshot.values.get(style_key) {
        Some(SettingValue::Text(text)) => {
            match allowed.iter().find(|(name, _)| *name == text.as_str()) {
                Some((_, Style::None)) => Style::None,
                Some((_, style)) => *style,
                None => default,
            }
        }
        _ => default,
    }
}

/// Maps `animationsEnabled`, `animationSpeed` (hundredths), and the per-effect styles.
/// `animationsEnabled=false` is applied by the timeline (`Settings::enabled`), not by the styles.
/// Missing or mistyped keys keep the default.
#[must_use]
pub fn settings_from_snapshot(snapshot: &SettingsSnapshot) -> Settings {
    let enabled = match snapshot.values.get("animationsEnabled") {
        Some(SettingValue::Boolean(value)) => *value,
        _ => Settings::DEFAULT.enabled,
    };
    let speed = match snapshot.values.get("animationSpeed") {
        Some(SettingValue::Integer(value)) => {
            u16::try_from(*value).unwrap_or(Settings::DEFAULT.speed)
        }
        _ => Settings::DEFAULT.speed,
    };
    let group = |enabled_key: &str, style_key: &str, default: Style, allowed: &[(&str, Style)]| {
        style_from_snapshot(snapshot, enabled_key, style_key, default, allowed)
    };
    let minimize_styles = [
        ("none", Style::None),
        ("fade", Style::Fade),
        ("magic-lamp", Style::MagicLamp),
    ];
    let open_styles = [
        ("none", Style::None),
        ("scale", Style::Scale),
        ("fade", Style::Fade),
    ];
    let maximize_styles = [
        ("none", Style::None),
        ("fade", Style::Fade),
        ("scale", Style::Scale),
        // Legacy persisted spelling of Scale (not the minimize Magic Lamp warp).
        ("magic-lamp", Style::Scale),
    ];
    // Close has no separate enabled toggle; the global `animationsEnabled` gate covers it.
    let close = match snapshot.values.get("windowCloseAnimation") {
        Some(SettingValue::Text(text)) => match text.as_str() {
            "none" => Style::None,
            "fade" => Style::Fade,
            _ => Settings::DEFAULT.close,
        },
        _ => Settings::DEFAULT.close,
    };
    Settings {
        enabled,
        speed,
        minimize: group(
            "minimizeAnimationEnabled",
            "minimizeAnimation",
            Settings::DEFAULT.minimize,
            &minimize_styles,
        ),
        maximize: group(
            "maximizeAnimationEnabled",
            "maximizeAnimation",
            Settings::DEFAULT.maximize,
            &maximize_styles,
        ),
        open: group(
            "windowOpenAnimationEnabled",
            "windowOpenAnimation",
            Settings::DEFAULT.open,
            &open_styles,
        ),
        close,
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

/// Reject a stale panel target on a different output before using it for a flight.
#[must_use]
pub fn icon_target_matches_output(icon: Rect, output: Rect) -> bool {
    if icon.width <= 0 || icon.height <= 0 || output.width <= 0 || output.height <= 0 {
        return false;
    }
    let center_x = i64::from(icon.x) * 2 + i64::from(icon.width);
    let center_y = i64::from(icon.y) * 2 + i64::from(icon.height);
    2 * i64::from(output.x) <= center_x
        && center_x < 2 * (i64::from(output.x) + i64::from(output.width))
        && 2 * i64::from(output.y) <= center_y
        && center_y < 2 * (i64::from(output.y) + i64::from(output.height))
}

/// Overlay warp parameter for a flight progress value. A mirrored overlay (restore) plays
/// target→source, so progress 0 starts at the target. Reversal keeps the overlay continuous
/// because the mirror flag never changes after the overlay is opened.
#[must_use]
pub fn overlay_t(progress: f32, mirrored: bool) -> f64 {
    let p = f64::from(progress.clamp(0.0, 1.0));
    if mirrored { 1.0 - p } else { p }
}

/// Window-open start rect: `outer` scaled to `OPEN_START_SCALE` about its centre. The open warp
/// runs from this rect (mirrored, so it is the warp target) up to `outer`.
#[must_use]
pub fn open_start_rect(outer: Rect) -> Rect {
    let width = ((f64::from(outer.width) * OPEN_START_SCALE).round() as i32).max(1);
    let height = ((f64::from(outer.height) * OPEN_START_SCALE).round() as i32).max(1);
    Rect {
        x: outer.x + (outer.width - width) / 2,
        y: outer.y + (outer.height - height) / 2,
        width,
        height,
    }
}

/// Window-open scale at the start of the flight, as a fraction of the settled size.
const OPEN_START_SCALE: f64 = 0.6;

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

/// A framed window may have painted its titlebar but not its client area yet.
/// Treat a wholly uniform client interior as not ready for a transition capture.
/// This deliberately skips a cosmetic open animation for blank-content windows
/// rather than scaling a titlebar over an empty rectangle and revealing content
/// abruptly at the end of the flight.
#[must_use]
pub fn is_unpainted_client_capture(
    argb: &[u8],
    width: u32,
    height: u32,
    titlebar_height: u32,
) -> bool {
    let width = width as usize;
    let height = height as usize;
    let top = titlebar_height as usize;
    if width == 0 || height <= top || argb.len() != width.saturating_mul(height).saturating_mul(4) {
        return true;
    }
    let area = &argb[top * width * 4..];
    let Some(first) = area.get(..4) else {
        return true;
    };
    // Sample across the client area without scanning entire high-resolution
    // frames per Expose. A partial painted client is never mistaken for ready
    // merely because the titlebar buttons already differ from its background.
    let stride = (area.len() / 4 / 4096).max(1);
    !area
        .chunks_exact(4)
        .step_by(stride)
        .any(|pixel| pixel != first)
}

/// Pending window-open effects. A managed window arms here when it is mapped; the effect
/// starts on a later Expose once the capture is non-blank. Arming never delays the map.
#[derive(Debug, Default)]
pub struct OpenGate {
    /// `(client, remaining capture attempts)`. Holds a handful of entries at most, so a
    /// linear `Vec` beats a hash map here.
    armed: Vec<(u32, u8)>,
}

impl OpenGate {
    pub fn arm(&mut self, client: u32) {
        self.clear(client);
        self.armed.push((client, OPEN_CAPTURE_ATTEMPTS));
    }

    #[must_use]
    pub fn is_armed(&self, client: u32) -> bool {
        self.armed.iter().any(|(c, _)| *c == client)
    }

    /// Records one capture attempt. Returns `true` when the capture is usable; the client is
    /// then disarmed and the caller starts the effect. A blank capture spends one attempt, and
    /// the last one disarms without starting, so a window that never paints is not retried.
    pub fn attempt(&mut self, client: u32, blank: bool) -> bool {
        let Some(index) = self.armed.iter().position(|(c, _)| *c == client) else {
            return false;
        };
        if !blank {
            self.armed.swap_remove(index);
            return true;
        }
        let left = &mut self.armed[index].1;
        *left -= 1;
        if *left == 0 {
            self.armed.swap_remove(index);
        }
        false
    }

    /// Drops the pending effect for `client` (input to it, unmap, or destroy).
    pub fn clear(&mut self, client: u32) {
        self.armed.retain(|(c, _)| *c != client);
    }
}

/// How long a finished overlay stays above its remapped window, so the client's first repaint
/// shows through instead of an unpainted frame.
pub const LINGER_MS: u64 = 250;

/// Finished overlays held above a remapped window until a deadline. Pure: callers inject `now`.
#[derive(Debug)]
pub struct Linger<T> {
    /// `(client, value, deadline)`; a handful of live overlays at most.
    held: Vec<(u32, T, Instant)>,
}

impl<T> Default for Linger<T> {
    fn default() -> Self {
        Self { held: Vec::new() }
    }
}

impl<T> Linger<T> {
    /// Holds `value` for `client` until `now` plus [`LINGER_MS`], replacing any earlier hold.
    pub fn hold(&mut self, client: u32, value: T, now: Instant) {
        self.remove(client);
        self.held
            .push((client, value, now + Duration::from_millis(LINGER_MS)));
    }

    /// Drops the hold for `client` at once (input to it, unmap, or destroy).
    pub fn remove(&mut self, client: u32) {
        self.held.retain(|(c, _, _)| *c != client);
    }

    /// Drops every hold whose deadline is at or before `now` and returns those clients.
    pub fn expire(&mut self, now: Instant) -> Vec<u32> {
        let mut expired = Vec::new();
        self.held.retain(|(client, _, deadline)| {
            let live = now < *deadline;
            if !live {
                expired.push(*client);
            }
            live
        });
        expired
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
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
    /// Creates an animator. `xrender_transform` is the `XRender` transform capability;
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
        self.style(effect) != Style::None
    }

    /// Visual style the current settings give `effect`.
    #[must_use]
    pub fn style(&self, effect: Effect) -> Style {
        effect_style(self.settings, effect)
    }

    fn base_ms(effect: Effect) -> u32 {
        match effect {
            Effect::Minimize | Effect::Restore => MINIMIZE_MS,
            Effect::Maximize | Effect::Unmaximize => MAXIMIZE_MS,
            Effect::Open => OPEN_MS,
            Effect::Close => CLOSE_MS,
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
        let easing = if matches!(effect, Effect::Maximize | Effect::Unmaximize) {
            Easing::Linear
        } else {
            Easing::Smoothstep
        };
        match self.timeline.start(id, effect, now, base, easing) {
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
        let mut out = Vec::with_capacity(self.flights.len());
        self.flights.retain(|flight| {
            if let Some(progress) = timeline.progress(flight.id, now) {
                out.push(Tick::Frame {
                    client: flight.client,
                    progress,
                });
                true
            } else {
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

    const ON: Settings = Settings::DEFAULT;

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
        let mut a = Animator::new(
            true,
            Settings {
                open: Style::Scale,
                ..ON
            },
        );
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
        let mut a = Animator::new(
            true,
            Settings {
                enabled: false,
                ..ON
            },
        );
        assert_eq!(a.start(7, Effect::Minimize, t0), Begin::Immediate);
        assert!(!a.is_active());
        assert!(a.tick(t0).is_empty());
    }

    #[test]
    fn per_effect_flag_gates_only_that_effect() {
        let t0 = Instant::now();
        let mut a = Animator::new(
            true,
            Settings {
                maximize: Style::None,
                ..ON
            },
        );
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
            other @ Tick::Complete { .. } => panic!("expected frame, got {other:?}"),
        };
        a.start(7, Effect::Restore, forward_end);
        let p_back = match a.tick(forward_end + Duration::from_millis(1))[0] {
            Tick::Frame { progress, .. } => progress,
            other @ Tick::Complete { .. } => panic!("expected frame, got {other:?}"),
        };
        assert!(p_back <= p_forward, "reverse must not jump forward");
    }

    #[test]
    fn minimize_target_prefers_icon_geometry() {
        let out = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let icon = Rect {
            x: 40,
            y: 1050,
            width: 32,
            height: 24,
        };
        assert_eq!(minimize_target(Some(icon), out), icon);
    }

    #[test]
    fn minimize_target_ignores_icon_geometry_from_another_output() {
        let left = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let right = Rect {
            x: 1920,
            y: 0,
            width: 1280,
            height: 720,
        };
        let left_icon = Rect {
            x: 80,
            y: 1036,
            width: 40,
            height: 40,
        };
        assert!(icon_target_matches_output(left_icon, left));
        assert!(!icon_target_matches_output(left_icon, right));
        assert_eq!(
            minimize_target(
                icon_target_matches_output(left_icon, right).then_some(left_icon),
                right
            ),
            Rect {
                x: 2560,
                y: 719,
                width: 1,
                height: 1
            },
        );
    }

    #[test]
    fn minimize_target_fallback_is_taskbar_center_not_origin() {
        let out = Rect {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let empty = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
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
        snapshot
            .values
            .insert("animationsEnabled".into(), SettingValue::Boolean(false));
        snapshot
            .values
            .insert("animationSpeed".into(), SettingValue::Integer(50));
        snapshot.values.insert(
            "maximizeAnimationEnabled".into(),
            SettingValue::Boolean(false),
        );
        let mapped = settings_from_snapshot(&snapshot);
        assert!(!mapped.enabled);
        assert_eq!(mapped.speed, 50);
        assert_eq!(mapped.maximize, Style::None);
        assert_eq!(
            mapped.minimize,
            Style::MagicLamp,
            "missing keys keep the default"
        );
        assert_eq!(
            mapped.open,
            Style::None,
            "window-open animation defaults off"
        );
        assert_eq!(
            settings_from_snapshot(&SettingsSnapshot::new(1)),
            Settings::DEFAULT
        );
    }

    #[test]
    fn linger_holds_until_deadline_and_remove_drops_early() {
        let t0 = Instant::now();
        let mut linger = Linger::default();
        linger.hold(7, "overlay", t0);
        linger.expire(t0 + Duration::from_millis(LINGER_MS - 1));
        assert!(
            !linger.is_empty(),
            "the overlay stays up until the client has had time to paint"
        );
        linger.expire(t0 + Duration::from_millis(LINGER_MS));
        assert!(linger.is_empty(), "the hold ends at its deadline");
        linger.hold(7, "overlay", t0);
        linger.remove(7);
        assert!(
            linger.is_empty(),
            "input, unmap or destroy drops the hold at once"
        );
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
        let mut off = Animator::new(
            true,
            Settings {
                maximize: Style::None,
                ..ON
            },
        );
        assert_eq!(off.start(7, Effect::Unmaximize, t0), Begin::Immediate);
        let mut a = Animator::new(
            true,
            Settings {
                maximize: Style::Scale,
                ..ON
            },
        );
        assert_eq!(a.start(7, Effect::Unmaximize, t0), Begin::Started);
        assert_eq!(a.running(7), Some(Effect::Unmaximize));
        let late = t0 + Duration::from_secs(5);
        assert_eq!(completes(&a.tick(late)), vec![(7, Effect::Unmaximize)]);
        assert!(completes(&a.tick(late)).is_empty(), "no second commit");
    }

    #[test]
    fn maximize_and_unmaximize_do_not_reverse_each_other() {
        let t0 = Instant::now();
        let mut a = Animator::new(
            true,
            Settings {
                maximize: Style::Scale,
                ..ON
            },
        );
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
        let mut a = Animator::new(
            true,
            Settings {
                open: Style::Scale,
                ..ON
            },
        );
        let mut gate = OpenGate::default();
        gate.arm(7);
        assert!(gate.is_armed(7));
        assert_eq!(
            a.running(7),
            None,
            "arming must not start a flight or delay the map"
        );
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
        assert!(
            !gate.is_armed(1),
            "a never-painting window is not retried forever"
        );
        assert!(
            !gate.attempt(1, false),
            "no pending effect after exhaustion"
        );
        gate.arm(2);
        gate.clear(2);
        assert!(
            !gate.attempt(2, false),
            "input or unmap before the expose cancels the open"
        );
    }

    #[test]
    fn open_wants_requires_enabled_capable_and_flag() {
        let enabled = Settings {
            open: Style::Scale,
            ..Settings::DEFAULT
        };
        let on = Animator::new(true, enabled);
        assert!(on.wants(Effect::Open));
        let flag_off = Animator::new(
            true,
            Settings {
                open: Style::None,
                ..enabled
            },
        );
        assert!(!flag_off.wants(Effect::Open));
        let no_cap = Animator::new(false, enabled);
        assert!(!no_cap.wants(Effect::Open));
        let disabled = Animator::new(
            true,
            Settings {
                enabled: false,
                ..enabled
            },
        );
        assert!(!disabled.wants(Effect::Open));
    }

    #[test]
    fn open_start_rect_is_smaller_and_centred_on_the_window() {
        let outer = Rect {
            x: 300,
            y: 200,
            width: 300,
            height: 200,
        };
        let start = open_start_rect(outer);
        assert!(
            start.width < outer.width && start.height < outer.height,
            "scales up from smaller"
        );
        assert!(
            f64::from(start.width) < f64::from(outer.width) * 0.9,
            "below the 0.9x threshold"
        );
        assert_eq!(
            start.x + start.width / 2,
            outer.x + outer.width / 2,
            "horizontally centred"
        );
        assert_eq!(
            start.y + start.height / 2,
            outer.y + outer.height / 2,
            "vertically centred"
        );
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};

    fn snapshot(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in entries {
            snapshot.values.insert((*key).into(), value.clone());
        }
        snapshot
    }

    fn text(value: &str) -> SettingValue {
        SettingValue::Text(value.to_owned())
    }

    #[test]
    fn open_capture_requires_client_pixels_not_only_titlebar() {
        let (w, h, top) = (8u32, 8u32, 2u32);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        // The frame titlebar is already colorful while the application body
        // has not painted; this must NOT start a partially filled scale.
        pixels[0..4].copy_from_slice(&[255, 60, 70, 255]);
        assert!(!is_blank_capture(&pixels));
        assert!(is_unpainted_client_capture(&pixels, w, h, top));
        let offset = ((top + 2) * w * 4 + 4 * 4) as usize;
        pixels[offset..offset + 4].copy_from_slice(&[100, 200, 50, 255]);
        assert!(!is_unpainted_client_capture(&pixels, w, h, top));
    }

    #[test]
    fn missing_style_keys_keep_defaults() {
        let mapped = settings_from_snapshot(&SettingsSnapshot::new(1));
        assert_eq!(mapped, Settings::DEFAULT);
        assert_eq!(mapped.minimize, Style::MagicLamp);
        assert_eq!(mapped.maximize, Style::None);
        assert_eq!(mapped.open, Style::None);
    }

    #[test]
    fn each_effect_parses_its_own_style_values() {
        let mapped = settings_from_snapshot(&snapshot(&[
            ("minimizeAnimation", text("fade")),
            ("maximizeAnimation", text("none")),
            ("windowOpenAnimation", text("fade")),
        ]));
        assert_eq!(mapped.minimize, Style::Fade);
        assert_eq!(mapped.maximize, Style::None);
        assert_eq!(mapped.open, Style::Fade);

        let mapped = settings_from_snapshot(&snapshot(&[
            ("minimizeAnimation", text("magic-lamp")),
            ("maximizeAnimation", text("fade")),
            ("windowOpenAnimation", text("none")),
        ]));
        assert_eq!(mapped.minimize, Style::MagicLamp);
        assert_eq!(mapped.maximize, Style::Fade);
        assert_eq!(mapped.open, Style::None);
    }

    #[test]
    fn style_not_offered_for_an_effect_keeps_its_default() {
        let mapped = settings_from_snapshot(&snapshot(&[
            ("minimizeAnimation", text("scale")),
            ("windowOpenAnimation", text("magic-lamp")),
            ("maximizeAnimation", SettingValue::Integer(3)),
        ]));
        assert_eq!(mapped.minimize, Style::MagicLamp);
        assert_eq!(mapped.open, Style::None);
        assert_eq!(mapped.maximize, Style::None);
    }

    #[test]
    fn legacy_enabled_flag_false_forces_none_over_style() {
        let mapped = settings_from_snapshot(&snapshot(&[
            ("minimizeAnimationEnabled", SettingValue::Boolean(false)),
            ("minimizeAnimation", text("fade")),
            ("maximizeAnimationEnabled", SettingValue::Boolean(false)),
            ("maximizeAnimation", text("fade")),
            ("windowOpenAnimationEnabled", SettingValue::Boolean(false)),
            ("windowOpenAnimation", text("fade")),
        ]));
        assert_eq!(mapped.minimize, Style::None);
        assert_eq!(mapped.open, Style::None);
        assert_eq!(mapped.maximize, Style::None);
    }

    #[test]
    fn master_flag_false_is_timeline_gate_and_speed_is_kept() {
        let mapped = settings_from_snapshot(&snapshot(&[
            ("animationsEnabled", SettingValue::Boolean(false)),
            ("animationSpeed", SettingValue::Integer(50)),
            ("minimizeAnimation", text("fade")),
        ]));
        assert!(!mapped.enabled);
        assert_eq!(mapped.speed, 50);
        assert_eq!(mapped.minimize, Style::Fade);
        assert!(!Animator::new(true, mapped).wants(Effect::Minimize));
    }

    #[test]
    fn effect_style_selects_the_group_for_each_effect() {
        let settings = Settings {
            minimize: Style::Fade,
            maximize: Style::MagicLamp,
            open: Style::None,
            ..Settings::DEFAULT
        };
        assert_eq!(effect_style(settings, Effect::Minimize), Style::Fade);
        assert_eq!(effect_style(settings, Effect::Restore), Style::Fade);
        assert_eq!(effect_style(settings, Effect::Maximize), Style::MagicLamp);
        assert_eq!(effect_style(settings, Effect::Unmaximize), Style::MagicLamp);
        assert_eq!(effect_style(settings, Effect::Open), Style::None);
    }

    #[test]
    fn close_style_defaults_to_fade_and_accepts_only_none_or_fade() {
        assert_eq!(settings_from_snapshot(&snapshot(&[])).close, Style::Fade);
        let none = settings_from_snapshot(&snapshot(&[("windowCloseAnimation", text("none"))]));
        assert_eq!(none.close, Style::None);
        assert_eq!(effect_style(none, Effect::Close), Style::None);
        let fade = settings_from_snapshot(&snapshot(&[("windowCloseAnimation", text("fade"))]));
        assert_eq!(effect_style(fade, Effect::Close), Style::Fade);
        for not_offered in ["scale", "magic-lamp", "Fade", ""] {
            let mapped =
                settings_from_snapshot(&snapshot(&[("windowCloseAnimation", text(not_offered))]));
            assert_eq!(mapped.close, Style::Fade, "{not_offered} keeps the default");
        }
    }

    #[test]
    fn close_is_gated_by_global_flag_and_speed_only() {
        let disabled = settings_from_snapshot(&snapshot(&[
            ("animationsEnabled", SettingValue::Boolean(false)),
            ("windowCloseAnimation", text("fade")),
        ]));
        let t0 = Instant::now();
        let mut off = Animator::new(true, disabled);
        assert!(!off.wants(Effect::Close));
        assert_eq!(off.start(7, Effect::Close, t0), Begin::Immediate);

        let none = settings_from_snapshot(&snapshot(&[("windowCloseAnimation", text("none"))]));
        assert_eq!(
            Animator::new(true, none).start(7, Effect::Close, t0),
            Begin::Immediate
        );

        assert_eq!(
            Animator::new(false, Settings::DEFAULT).start(7, Effect::Close, t0),
            Begin::Immediate
        );

        let slow =
            settings_from_snapshot(&snapshot(&[("animationSpeed", SettingValue::Integer(50))]));
        let just_after_base = t0 + Duration::from_millis(u64::from(CLOSE_MS) + 1);
        let mut slow_animator = Animator::new(true, slow);
        assert_eq!(slow_animator.start(7, Effect::Close, t0), Begin::Started);
        let slow_ticks = slow_animator.tick(just_after_base);
        assert!(
            !slow_ticks
                .iter()
                .any(|tick| matches!(tick, Tick::Complete { .. })),
            "0.5x runs longer"
        );

        let mut normal = Animator::new(true, Settings::DEFAULT);
        assert_eq!(normal.start(7, Effect::Close, t0), Begin::Started);
        assert_eq!(
            normal.tick(just_after_base),
            vec![Tick::Complete {
                client: 7,
                effect: Effect::Close
            }]
        );
    }

    #[test]
    fn only_a_running_close_survives_unmanage() {
        assert!(survives_unmanage(Some(Effect::Close)));
        assert!(!survives_unmanage(Some(Effect::Minimize)));
        assert!(!survives_unmanage(None));
    }

    #[test]
    fn animator_refuses_effects_whose_style_is_none() {
        let settings = Settings {
            open: Style::None,
            minimize: Style::Fade,
            ..Settings::DEFAULT
        };
        let animator = Animator::new(true, settings);
        assert_eq!(animator.style(Effect::Minimize), Style::Fade);
        assert!(animator.wants(Effect::Minimize));
        assert_eq!(animator.style(Effect::Open), Style::None);
        assert!(!animator.wants(Effect::Open));
    }
}
