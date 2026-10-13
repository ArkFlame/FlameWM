use std::f64::consts::{PI, TAU};

use crate::colour::palette;
use crate::config::{Config, RIBBON_DEFAULT_SPEED_FACTOR, Rect, union_bounds};
use crate::profile::{LIFETIME_S, smoothstep};
use crate::rng::SplitMix64;

/// Fixed simulation step, seconds.
pub const DT: f64 = 1.0 / 120.0;
/// Ring capacity per ribbon; the 8.5 s lifetime at 60 Hz needs 511 samples.
pub const HISTORY_CAPACITY: usize = 512;
/// One history sample every 2 steps, i.e. 60 Hz.
const SAMPLE_EVERY_STEPS: u64 = 2;
const MAX_CATCH_UP_STEPS: usize = 4;
/// Gaps longer than this rebuild the state from the seed instead of stepping through them.
const REBUILD_AFTER_S: f64 = 2.0;
const EDGE_GAIN: f64 = 1.35;
/// Extra attraction multiplier reached outside the viewport (see `attraction`).
const OUTSIDE_GAIN: f64 = 6.0;
/// Heading wobble terms as `(frequency Hz, amplitude rad)`.
const WOBBLE: [(f64, f64); 3] = [(0.065, 0.75), (0.17, 0.34), (0.31, 0.18)];

/// One history sample: centreline position plus pseudo-depth and twist at sample time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub pos: [f64; 2],
    pub z: f64,
    pub beta: f64,
    pub t: f64,
}

/// Fixed-capacity ring of samples, oldest first. Samples older than `LIFETIME_S` are dropped.
#[derive(Clone, Debug)]
pub struct History {
    buf: Vec<Sample>,
    head: usize,
    len: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            buf: vec![Sample::default(); HISTORY_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// The `k`-th oldest sample (`k = 0` is oldest). Panics when `k >= len()`.
    pub fn get(&self, k: usize) -> Sample {
        assert!(k < self.len, "history index out of range");
        self.buf[(self.head + k) % self.buf.len()]
    }

    /// Appends `s`, evicting the oldest sample when full or older than `LIFETIME_S` relative to `s`.
    pub fn push(&mut self, s: Sample) {
        let cap = self.buf.len();
        if self.len == cap {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
        let idx = (self.head + self.len) % cap;
        self.buf[idx] = s;
        self.len += 1;
        while self.len > 1 && s.t - self.get(0).t > LIFETIME_S {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

/// One ribbon: heading/position state plus its trail history.
#[derive(Clone, Debug)]
pub struct Stream {
    pos: [f64; 2],
    theta: f64,
    omega: f64,
    speed: f64,
    phase: [f64; 3],
    psi: f64,
    chi: f64,
    history: History,
}

impl Stream {
    fn new(
        rng: &mut SplitMix64,
        dims: (f64, f64),
        scale: f64,
        speed_factor: f64,
        now: f64,
    ) -> Self {
        let (w, h) = dims;
        let speed = 145.0 * speed_factor * scale * (0.75 + 0.50 * rng.unit());
        let magnitude = 0.16 + 0.30 * rng.unit();
        let omega = if rng.unit() < 0.5 { -magnitude } else { magnitude };
        let phase = [TAU * rng.unit(), TAU * rng.unit(), TAU * rng.unit()];
        let psi = TAU * rng.unit();
        let chi = TAU * rng.unit();
        let pos = [w * (0.25 + 0.5 * rng.unit()), h * (0.25 + 0.5 * rng.unit())];
        let theta = TAU * rng.unit();
        let mut history = History::new();
        history.push(sample(pos, psi, chi, now));
        Self {
            pos,
            theta,
            omega,
            speed,
            phase,
            psi,
            chi,
            history,
        }
    }

    pub fn position(&self) -> [f64; 2] {
        self.pos
    }

    pub fn heading(&self) -> f64 {
        self.theta
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Midpoint (RK2-style) step: `omega_a` at the start, then the heading at the midpoint.
    fn step(&mut self, t: f64, dims: (f64, f64)) {
        let omega_a = self.ang(self.theta, self.pos, t, dims);
        let theta_half = self.theta + 0.5 * DT * omega_a;
        self.pos[0] += DT * self.speed * theta_half.cos();
        self.pos[1] += DT * self.speed * theta_half.sin();
        self.theta += DT * self.ang(theta_half, self.pos, t + 0.5 * DT, dims);
    }

    /// Heading rate `dtheta/dt`: drift + wobble derivative + edge attraction toward the centre.
    fn ang(&self, theta: f64, pos: [f64; 2], t: f64, dims: (f64, f64)) -> f64 {
        let (w, h) = dims;
        let mut rate = self.omega;
        for (k, &(f, a)) in WOBBLE.iter().enumerate() {
            rate += TAU * f * a * (TAU * f * t + self.phase[k]).cos();
        }
        let theta_c = (h / 2.0 - pos[1]).atan2(w / 2.0 - pos[0]);
        rate += EDGE_GAIN * attraction(pos, dims) * wrap_angle(theta_c - theta).sin();
        rate
    }
}

/// Edge attraction `B(P) = 1 - smoothstep(0, 0.18 D, d)`, `D = min(W, H)`, `d` the distance to
/// the nearest edge inside the viewport.
///
/// Outside the viewport `B` ramps smoothly from 1 up to 7 over `0.12 D` of distance outside.
/// Without this ramp a ribbon heading straight out has a zero turn rate (`sin` of the antipode
/// is 0, an unstable equilibrium) and can drift far away before turning back.
fn attraction(pos: [f64; 2], dims: (f64, f64)) -> f64 {
    let (w, h) = dims;
    let [x, y] = pos;
    let min_dim = w.min(h);
    if (0.0..=w).contains(&x) && (0.0..=h).contains(&y) {
        let d = x.min(w - x).min(y).min(h - y);
        1.0 - smoothstep(0.0, 0.18 * min_dim, d)
    } else {
        let dx = (-x).max(x - w).max(0.0);
        let dy = (-y).max(y - h).max(0.0);
        1.0 + OUTSIDE_GAIN * smoothstep(0.0, 0.12 * min_dim, dx.hypot(dy))
    }
}

fn wrap_angle(x: f64) -> f64 {
    (x + PI).rem_euclid(TAU) - PI
}

fn z_at(t: f64, psi: f64) -> f64 {
    (1.0 + (TAU * 0.045 * t + psi).sin()) / 2.0
}

fn beta_at(t: f64, chi: f64) -> f64 {
    0.6 * (TAU * 0.12 * t + chi).sin()
}

fn sample(pos: [f64; 2], psi: f64, chi: f64, t: f64) -> Sample {
    Sample {
        pos,
        z: z_at(t, psi),
        beta: beta_at(t, chi),
        t,
    }
}

/// The Ribbons simulation over a viewport (the union bounds of all span rects).
#[derive(Clone, Debug)]
pub struct Ribbons {
    config: Config,
    width: u32,
    height: u32,
    scale: f64,
    speed_factor: f64,
    /// Output rects in viewport-local coordinates; empty when unmasked.
    mask: Vec<Rect>,
    masked: bool,
    colours: Vec<[u8; 3]>,
    streams: Vec<Stream>,
    origin_t: f64,
    steps: u64,
    acc: f64,
    last_t: f64,
}

impl Ribbons {
    /// Creates a simulation at time `now` (seconds). Config values are clamped.
    ///
    /// With several `span_rects` the domain is their union bounds and `covers` masks the gaps;
    /// for independent outputs create one `Ribbons` per output instead.
    pub fn new(config: Config, now: f64) -> Self {
        Self::new_with_speed_factor(config, now, RIBBON_DEFAULT_SPEED_FACTOR)
    }

    /// Like `new`, but multiplies the base ribbon speed by `factor` instead of the default.
    /// `factor` does not change RNG draw order, so the same seed gives the same paths, scaled.
    pub fn new_with_speed_factor(config: Config, now: f64, factor: f64) -> Self {
        let factor = if factor.is_finite() {
            factor.clamp(0.0, 4.0)
        } else {
            RIBBON_DEFAULT_SPEED_FACTOR
        };
        let config = config.clamped();
        let bounds = union_bounds(&config.span_rects);
        let mask: Vec<Rect> = config
            .span_rects
            .iter()
            .map(|r| Rect {
                x: r.x - bounds.x,
                y: r.y - bounds.y,
                ..*r
            })
            .collect();
        let local_bounds = Rect { x: 0, y: 0, ..bounds };
        let masked = !mask.is_empty() && !mask.contains(&local_bounds);
        let width = bounds.w;
        let height = bounds.h;
        let scale = (f64::from(width) / 1280.0)
            .min(f64::from(height) / 720.0)
            .clamp(0.60, 2.50);
        let colours = palette(config.num, config.seed);
        let mut this = Self {
            config,
            width,
            height,
            scale,
            speed_factor: factor,
            mask,
            masked,
            colours,
            streams: Vec::new(),
            origin_t: now,
            steps: 0,
            acc: 0.0,
            last_t: now,
        };
        this.reseed(now);
        this
    }

    /// Rebuilds all streams from the seed at time `now`, with empty catch-up state.
    fn reseed(&mut self, now: f64) {
        let dims = (f64::from(self.width), f64::from(self.height));
        self.origin_t = now;
        self.steps = 0;
        self.acc = 0.0;
        self.last_t = now;
        self.streams = (0..self.config.num)
            .map(|i| {
                let mix = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let mut rng = SplitMix64::new(self.config.seed.wrapping_add(mix));
                Stream::new(&mut rng, dims, self.scale, self.speed_factor, now)
            })
            .collect();
    }

    /// Advances to absolute time `t_seconds` using fixed `DT` steps with an accumulator.
    ///
    /// At most 4 steps run per call and any remaining backlog is dropped. A gap over 2 s, or
    /// a backwards step, rebuilds from the seed (`reseed`) instead of stepping through it.
    pub fn step_to(&mut self, t_seconds: f64) {
        if !t_seconds.is_finite() {
            return;
        }
        let elapsed = t_seconds - self.last_t;
        if elapsed < 0.0 {
            return;
        }
        if elapsed > REBUILD_AFTER_S {
            self.reseed(t_seconds);
            return;
        }
        self.last_t = t_seconds;
        self.acc += elapsed;
        let mut n = 0;
        while self.acc >= DT && n < MAX_CATCH_UP_STEPS {
            self.step_once();
            self.acc -= DT;
            n += 1;
        }
        if self.acc >= DT {
            self.acc %= DT;
        }
    }

    fn step_once(&mut self) {
        let t_start = self.origin_t + self.steps as f64 * DT;
        self.steps += 1;
        let dims = (f64::from(self.width), f64::from(self.height));
        for s in &mut self.streams {
            s.step(t_start, dims);
        }
        if self.steps % SAMPLE_EVERY_STEPS == 0 {
            let t = self.time();
            for s in &mut self.streams {
                let sample = sample(s.pos, s.psi, s.chi, t);
                s.history.push(sample);
            }
        }
    }

    /// Current simulation time in seconds.
    pub fn time(&self) -> f64 {
        self.origin_t + self.steps as f64 * DT
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Screen scale `S = clamp(min(W/1280, H/720), 0.60, 2.50)`.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Configured ribbon width in px at 1280x720 (already clamped).
    pub fn width_px(&self) -> f64 {
        self.config.width_px
    }

    pub fn num_ribbons(&self) -> usize {
        self.streams.len()
    }

    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    /// SRGB colour of ribbon `i`.
    pub fn colour(&self, i: usize) -> [u8; 3] {
        self.colours[i % self.colours.len()]
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    /// Whether viewport pixel `(x, y)` lies inside an output rect. Always true when unmasked.
    pub fn covers(&self, x: u32, y: u32) -> bool {
        !self.masked
            || self
                .mask
                .iter()
                .any(|r| r.contains(i64::from(x), i64::from(y)))
    }

    /// Total allocated history capacity across streams, in samples.
    pub fn history_capacity(&self) -> usize {
        self.streams.iter().map(|s| s.history.capacity()).sum()
    }

    #[cfg(test)]
    pub(crate) fn clear_histories(&mut self) {
        for s in &mut self.streams {
            s.history.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(num: usize, seed: u64) -> Config {
        Config {
            num,
            seed,
            ..Config::default()
        }
    }

    fn dims(r: &Ribbons) -> (f64, f64) {
        (f64::from(r.width()), f64::from(r.height()))
    }

    #[test]
    fn fixed_dt_run_is_deterministic_for_a_seed() {
        let mut a = Ribbons::new(cfg(6, 42), 0.0);
        let mut b = Ribbons::new(cfg(6, 42), 0.0);
        for k in 1..=900 {
            a.step_to(k as f64 / 60.0);
            b.step_to(k as f64 / 60.0);
        }
        for (x, y) in a.streams().iter().zip(b.streams()) {
            assert_eq!(x.position(), y.position());
            assert_eq!(x.heading(), y.heading());
        }
        let mut c = Ribbons::new(cfg(6, 43), 0.0);
        c.step_to(15.0);
        assert_ne!(a.streams()[0].position(), c.streams()[0].position());
    }

    #[test]
    fn heading_stays_finite_over_ten_minutes() {
        let mut r = Ribbons::new(cfg(12, 7), 0.0);
        for k in 1..=36_000 {
            r.step_to(k as f64 / 60.0);
        }
        for s in r.streams() {
            assert!(s.heading().is_finite() && s.position().iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn default_speed_is_the_base_times_the_default_factor() {
        let r = Ribbons::new(cfg(12, 5), 0.0);
        let base = 145.0 * RIBBON_DEFAULT_SPEED_FACTOR * r.scale();
        for s in r.streams() {
            assert!(s.speed() >= base * 0.75 && s.speed() <= base * 1.25, "speed {}", s.speed());
        }
    }

    #[test]
    fn no_teleport_step_is_bounded_by_speed() {
        let mut r = Ribbons::new(cfg(12, 3), 0.0);
        for _ in 0..(120 * 600) {
            let before: Vec<[f64; 2]> = r.streams().iter().map(Stream::position).collect();
            r.step_once();
            for (s, p0) in r.streams().iter().zip(before) {
                let p1 = s.position();
                let d = (p1[0] - p0[0]).hypot(p1[1] - p0[1]);
                assert!(d <= s.speed() * DT * 1.01, "step {d} exceeds v*dt");
            }
        }
    }

    #[test]
    fn edge_attraction_keeps_path_within_half_viewport() {
        for seed in 0..4u64 {
            let mut r = Ribbons::new(cfg(12, seed), 0.0);
            let (w, h) = dims(&r);
            let limit = 0.5 * w.min(h);
            for k in 1..=(60 * 600) {
                r.step_to(k as f64 / 60.0);
                for s in r.streams() {
                    let [x, y] = s.position();
                    let out_x = (-x).max(x - w).max(0.0);
                    let out_y = (-y).max(y - h).max(0.0);
                    assert!(out_x.hypot(out_y) <= limit, "seed {seed} strayed {out_x},{out_y}");
                }
            }
        }
    }

    #[test]
    fn speed_factor_is_sanitised_before_use() {
        let speed = |factor: f64| Ribbons::new_with_speed_factor(cfg(4, 1), 0.0, factor).speed_factor;
        assert_eq!(speed(f64::NAN), RIBBON_DEFAULT_SPEED_FACTOR);
        assert_eq!(speed(f64::INFINITY), RIBBON_DEFAULT_SPEED_FACTOR);
        assert_eq!(speed(f64::NEG_INFINITY), RIBBON_DEFAULT_SPEED_FACTOR);
        assert_eq!(speed(-2.0), 0.0);
        assert_eq!(speed(9.0), 4.0);
        assert_eq!(speed(1.0), 1.0);
    }

    #[test]
    fn history_ring_is_bounded_and_lifetime_pruned() {
        let mut hist = History::new();
        for k in 0..1000 {
            hist.push(Sample { pos: [0.0, 0.0], z: 0.5, beta: 0.0, t: k as f64 / 60.0 });
        }
        assert!(hist.len() <= HISTORY_CAPACITY);
        assert_eq!(hist.capacity(), HISTORY_CAPACITY);
        let newest = hist.get(hist.len() - 1).t;
        assert!(newest - hist.get(0).t <= LIFETIME_S + 1e-9);
        assert!(hist.len() >= 500, "keeps a full lifetime");
    }

    #[test]
    fn stream_count_matches_num_ribbons() {
        for &n in &[1usize, 6, 12] {
            let mut r = Ribbons::new(cfg(n, 5), 0.0);
            r.step_to(1.0);
            assert_eq!(r.num_ribbons(), n);
            assert!(r.streams().iter().all(|s| s.history().len() >= 2));
        }
    }

    #[test]
    fn memory_stays_bounded_over_ten_simulated_minutes() {
        let mut r = Ribbons::new(cfg(12, 11), 0.0);
        let cap = r.history_capacity();
        for k in 1..=36_000 {
            r.step_to(k as f64 / 60.0);
            assert_eq!(r.history_capacity(), cap);
        }
        for s in r.streams() {
            assert!(s.history().len() <= HISTORY_CAPACITY);
        }
    }

    #[test]
    fn long_pause_rebuilds_deterministically() {
        let mut a = Ribbons::new(cfg(6, 9), 0.0);
        let mut b = Ribbons::new(cfg(6, 9), 0.0);
        a.step_to(0.5);
        a.step_to(100.0);
        b.step_to(100.0);
        assert_eq!(a.streams()[0].position(), b.streams()[0].position());
        assert_eq!(a.time(), b.time());
    }

    #[test]
    fn catch_up_is_capped_at_four_steps_per_call() {
        let mut r = Ribbons::new(cfg(1, 2), 0.0);
        r.step_to(1.0);
        assert_eq!(r.steps, 4, "a 1 s frame runs only the 4-step cap");
        let before = r.steps;
        r.step_to(1.0 + 1.0 / 60.0);
        assert!((1..=2).contains(&(r.steps - before)), "1/60 s is about two fixed steps");
    }

    #[test]
    fn attraction_grows_outside_and_vanishes_at_centre() {
        let d = (1280.0, 720.0);
        assert!(attraction([640.0, 360.0], d) < 1e-12);
        assert!(attraction([-5.0, 360.0], d) >= 1.0);
        assert!((attraction([0.0, 360.0], d) - 1.0).abs() < 1e-12);
        let mut prev = attraction([-1.0, 360.0], d);
        for i in 1..=40 {
            let b = attraction([-f64::from(i) * 10.0, 360.0], d);
            assert!(b >= prev - 1e-12, "outside attraction must not decrease");
            prev = b;
        }
        assert!((prev - 7.0).abs() < 1e-12, "saturates at 7x beyond 0.12 D");
    }
}
