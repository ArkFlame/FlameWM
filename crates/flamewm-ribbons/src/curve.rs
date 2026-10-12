use crate::profile::{coverage, ribbon_width};
use crate::sim::History;

/// Max chord deviation as a multiple of the screen scale `S` (px).
const TOLERANCE_SCALE: f64 = 0.75;
/// At most 4 subdivision levels per sample interval (2^4 = 16 pieces).
const MAX_PIECES: usize = 16;
/// Samples per Hermite knot span (60 Hz samples, so 15 Hz knots). Keeps segment count low.
const KNOT_STRIDE: usize = 8;
/// Tangent length below which the last tangent is retained.
const TANGENT_EPS: f64 = 1e-9;

/// Point on the ribbon centreline with its local width (px) and coverage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StripPoint {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub a: f32,
}

/// Cubic Hermite with `u` in `[0, 1]` per sample interval. Tangents `m0`, `m1` are in `u` units.
pub fn hermite(p0: [f64; 2], p1: [f64; 2], m0: [f64; 2], m1: [f64; 2], u: f64) -> [f64; 2] {
    let (u2, u3) = (u * u, u * u * u);
    let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
    let h10 = u3 - 2.0 * u2 + u;
    let h01 = -2.0 * u3 + 3.0 * u2;
    let h11 = u3 - u2;
    std::array::from_fn(|c| h00 * p0[c] + h10 * m0[c] + h01 * p1[c] + h11 * m1[c])
}

/// First derivative `C'(u)` of `hermite`.
#[cfg(test)]
pub fn hermite_deriv(p0: [f64; 2], p1: [f64; 2], m0: [f64; 2], m1: [f64; 2], u: f64) -> [f64; 2] {
    let u2 = u * u;
    let d00 = 6.0 * u2 - 6.0 * u;
    let d10 = 3.0 * u2 - 4.0 * u + 1.0;
    let d01 = -6.0 * u2 + 6.0 * u;
    let d11 = 3.0 * u2 - 2.0 * u;
    std::array::from_fn(|c| d00 * p0[c] + d10 * m0[c] + d01 * p1[c] + d11 * m1[c])
}

/// Second derivative `C''(u)` of `hermite`; linear in `u`.
fn hermite_second(p0: [f64; 2], p1: [f64; 2], m0: [f64; 2], m1: [f64; 2], u: f64) -> [f64; 2] {
    let d00 = 12.0 * u - 6.0;
    let d10 = 6.0 * u - 4.0;
    let d01 = 6.0 - 12.0 * u;
    let d11 = 6.0 * u - 2.0;
    std::array::from_fn(|c| d00 * p0[c] + d10 * m0[c] + d01 * p1[c] + d11 * m1[c])
}

fn norm(v: [f64; 2]) -> f64 {
    v[0].hypot(v[1])
}

/// Builds the centreline points of a trail, oldest to newest, into `out` (cleared first).
///
/// Tangents are central differences `(P[j+1] - P[j-1]) / 2`, one-sided at the ends, and the last
/// usable tangent is retained where `|C'| < eps`. Knot spans of `KNOT_STRIDE` samples are Hermite
/// curves whose tangents are scaled by the span length; each span is split into the fewest
/// power-of-two pieces (at most 16) whose chord deviation is at most `0.75 * scale` px.
/// `tangents` is scratch space reused across calls.
pub fn build_strip(
    history: &History,
    now: f64,
    scale: f64,
    width_px: f64,
    tangents: &mut Vec<[f64; 2]>,
    out: &mut Vec<StripPoint>,
) {
    out.clear();
    tangents.clear();
    let n = history.len();
    if n < 2 {
        return;
    }
    let pos = |j: usize| history.get(j).pos;
    let mut last = [0.0, 0.0];
    for j in 0..n {
        let raw = if j == 0 {
            sub(pos(1), pos(0))
        } else if j == n - 1 {
            sub(pos(n - 1), pos(n - 2))
        } else {
            scale2(sub(pos(j + 1), pos(j - 1)), 0.5)
        };
        if norm(raw) < TANGENT_EPS {
            tangents.push(last);
        } else {
            last = raw;
            tangents.push(raw);
        }
    }

    let tol = TOLERANCE_SCALE * scale;
    let mut j = 0;
    while j < n - 1 {
        // Knot span from sample `j` to `k`; the Hermite parameter runs over the span's intervals.
        let k = (j + KNOT_STRIDE).min(n - 1);
        let span = (k - j) as f64;
        let (a, b) = (history.get(j), history.get(k));
        let (p0, p1) = (a.pos, b.pos);
        let (m0, m1) = (scale2(tangents[j], span), scale2(tangents[k], span));
        let curvature = norm(hermite_second(p0, p1, m0, m1, 0.0))
            .max(norm(hermite_second(p0, p1, m0, m1, 1.0)));
        let mut pieces = 1usize;
        while pieces < MAX_PIECES && curvature / (8.0 * (pieces * pieces) as f64) > tol {
            pieces *= 2;
        }
        let wa = ribbon_width(a.z, a.beta, scale, width_px);
        let wb = ribbon_width(b.z, b.beta, scale, width_px);
        let aa = coverage(now - a.t, a.z);
        let ab = coverage(now - b.t, b.z);
        for i in 0..pieces {
            let u = i as f64 / pieces as f64;
            let c = hermite(p0, p1, m0, m1, u);
            out.push(StripPoint {
                x: c[0] as f32,
                y: c[1] as f32,
                w: lerp(wa, wb, u) as f32,
                a: lerp(aa, ab, u) as f32,
            });
        }
        j = k;
    }
    let last_sample = history.get(n - 1);
    out.push(StripPoint {
        x: last_sample.pos[0] as f32,
        y: last_sample.pos[1] as f32,
        w: ribbon_width(last_sample.z, last_sample.beta, scale, width_px) as f32,
        a: coverage(now - last_sample.t, last_sample.z) as f32,
    });
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale2(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Sample;

    #[test]
    fn hermite_endpoints_and_tangents_are_exact() {
        let p0 = [1.0, -2.0];
        let p1 = [4.0, 3.0];
        let m0 = [0.5, 2.0];
        let m1 = [-1.0, 0.25];
        assert_eq!(hermite(p0, p1, m0, m1, 0.0), p0);
        assert_eq!(hermite(p0, p1, m0, m1, 1.0), p1);
        let d0 = hermite_deriv(p0, p1, m0, m1, 0.0);
        let d1 = hermite_deriv(p0, p1, m0, m1, 1.0);
        for c in 0..2 {
            assert!((d0[c] - m0[c]).abs() < 1e-12);
            assert!((d1[c] - m1[c]).abs() < 1e-12);
        }
    }

    #[test]
    fn hermite_derivative_matches_finite_difference() {
        let (p0, p1, m0, m1) = ([0.0, 0.0], [3.0, 1.0], [1.0, 0.5], [0.2, -1.0]);
        let h = 1e-6;
        for &u in &[0.1, 0.5, 0.9] {
            let num = hermite(p0, p1, m0, m1, u + h);
            let base = hermite(p0, p1, m0, m1, u - h);
            let d = hermite_deriv(p0, p1, m0, m1, u);
            for c in 0..2 {
                assert!(((num[c] - base[c]) / (2.0 * h) - d[c]).abs() < 1e-5);
            }
        }
    }

    fn history_line(n: usize, step: f64) -> History {
        let mut h = History::new();
        for k in 0..n {
            h.push(Sample {
                pos: [k as f64 * step, 10.0],
                z: 0.5,
                beta: 0.0,
                t: k as f64 / 60.0,
            });
        }
        h
    }

    #[test]
    fn strip_of_straight_line_stays_on_line_and_ends_on_samples() {
        let h = history_line(20, 2.0);
        let (mut tangents, mut out) = (Vec::new(), Vec::new());
        build_strip(&h, 20.0 / 60.0, 1.0, 11.0, &mut tangents, &mut out);
        assert_eq!(out.len(), 4, "knots at samples 0, 8, 16 and 19");
        assert_eq!((out[0].x, out[0].y), (0.0, 10.0));
        let last = out.last().unwrap();
        assert!((last.x - 38.0).abs() < 1e-4 && (last.y - 10.0).abs() < 1e-4);
        assert!(out.iter().all(|p| (p.y - 10.0).abs() < 1e-4), "straight stays straight");
        assert!(out.iter().all(|p| p.w.is_finite() && p.a.is_finite()));
    }

    #[test]
    fn strip_is_empty_for_fewer_than_two_samples() {
        let h = history_line(1, 1.0);
        let (mut tangents, mut out) = (Vec::new(), Vec::new());
        build_strip(&h, 0.0, 1.0, 11.0, &mut tangents, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn stationary_samples_retain_finite_tangents() {
        let mut h = History::new();
        for k in 0..5 {
            h.push(Sample { pos: [5.0, 5.0], z: 0.2, beta: 0.1, t: k as f64 / 60.0 });
        }
        let (mut tangents, mut out) = (Vec::new(), Vec::new());
        build_strip(&h, 0.1, 1.0, 11.0, &mut tangents, &mut out);
        assert!(tangents.iter().all(|t| t[0].is_finite() && t[1].is_finite()));
        assert!(out.iter().all(|p| p.x == 5.0 && p.y == 5.0));
    }
}
