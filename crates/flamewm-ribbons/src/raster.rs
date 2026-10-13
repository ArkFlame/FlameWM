use crate::colour::{encode_table, to_linear};
use crate::curve::{StripPoint, build_strip};
use crate::profile::glow_terms;
use crate::sim::Ribbons;

/// Glow cut-off `(d / w)^2 < 24`, i.e. `d < 4.9w`, where the wide Gaussian term is below 0.003.
const GLOW_Q_MAX: f32 = 24.0;
const GLOW_LUT_LEN: usize = 1024;
const GLOW_LUT_SCALE: f32 = (GLOW_LUT_LEN as f32 - 1.0) / GLOW_Q_MAX;
/// Maximum white mix on the core, reached where the narrow term is 1.
const WHITE_MIX: f32 = 0.14;
const ENCODE_LEN: usize = 4096;
/// Pixels per block in the capsule splat inner loop.
const LANES: usize = 8;

/// Internal resolution: the frame is rendered at `1/factor` and bilinearly upsampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    Full,
    #[default]
    Half,
    Quarter,
}

impl Quality {
    /// Downscale factor applied to the viewport for the accumulation buffers.
    pub fn factor(self) -> usize {
        match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
        }
    }
}

/// CPU rasterizer with preallocated scratch buffers. Output is BGRA (`A = 255`).
///
/// Each ribbon is splatted segment by segment: a pixel's alpha is the maximum over its segments
/// (no double-counting at joints), and ribbons are combined by screen blend in linear light via a
/// running transmission product, `out = 1 - prod(1 - alpha * colour)`.
#[derive(Debug)]
pub struct Rasterizer {
    quality: Quality,
    glow: Vec<(f32, f32)>,
    encode: Vec<u8>,
    points: Vec<StripPoint>,
    tangents: Vec<[f64; 2]>,
    alpha: Vec<f32>,
    core: Vec<f32>,
    transmit: Vec<[f32; 3]>,
    low: Vec<[u8; 3]>,
    x_taps: Vec<(usize, usize, f32)>,
    y_taps: Vec<(usize, usize, f32)>,
    row: Vec<[f32; 3]>,
}

impl Rasterizer {
    pub fn new(quality: Quality) -> Self {
        let glow = (0..GLOW_LUT_LEN)
            .map(|i| {
                let q = f64::from(i as f32 / GLOW_LUT_SCALE);
                let (g, core) = glow_terms(q);
                (g as f32, core as f32)
            })
            .collect();
        Self {
            quality,
            glow,
            encode: encode_table(ENCODE_LEN),
            points: Vec::new(),
            tangents: Vec::new(),
            alpha: Vec::new(),
            core: Vec::new(),
            transmit: Vec::new(),
            low: Vec::new(),
            x_taps: Vec::new(),
            y_taps: Vec::new(),
            row: Vec::new(),
        }
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Renders `state` into `out`, which must be exactly `width * height * 4` bytes (BGRA).
    /// Scratch buffers are reused; allocation happens only when the viewport size changes.
    pub fn render(&mut self, state: &Ribbons, out: &mut [u8]) {
        self.render_ordered(state, out, [2, 1, 0]);
    }

    /// Same pixels as [`Rasterizer::render`], written as RGBA (`A = 255`) in one pass.
    pub fn render_rgba(&mut self, state: &Ribbons, out: &mut [u8]) {
        self.render_ordered(state, out, [0, 1, 2]);
    }

    /// `order` gives the source channel index (of linear RGB) stored at each output byte.
    fn render_ordered(&mut self, state: &Ribbons, out: &mut [u8], order: [usize; 3]) {
        let (w, h) = (state.width() as usize, state.height() as usize);
        assert_eq!(out.len(), w * h * 4, "output must be width*height*4 bytes");
        if w == 0 || h == 0 {
            return;
        }
        let k = self.quality.factor();
        let (lw, lh) = (w.div_ceil(k), h.div_ceil(k));
        let n = lw * lh;
        self.alpha.resize(n, 0.0);
        self.core.resize(n, 0.0);
        self.transmit.clear();
        self.transmit.resize(n, [1.0; 3]);
        self.low.clear();
        self.low.resize(n, [0; 3]);

        let inv = 1.0 / k as f32;
        let (scale, width_px, now) = (state.scale(), state.width_px(), state.time());
        for (i, stream) in state.streams().iter().enumerate() {
            build_strip(
                stream.history(),
                now,
                scale,
                width_px,
                &mut self.tangents,
                &mut self.points,
            );
            if self.points.len() < 2 {
                continue;
            }
            let col = to_linear(state.colour(i));
            splat_stream(
                &mut self.alpha,
                &mut self.core,
                &mut self.transmit,
                &self.points,
                &self.glow,
                (lw, lh),
                inv,
                col,
            );
        }

        let encode = &self.encode;
        for (dst, t) in self.low.iter_mut().zip(&self.transmit) {
            *dst = t.map(|v| {
                encode[((1.0 - v).clamp(0.0, 1.0) * (ENCODE_LEN - 1) as f32 + 0.5) as usize]
            });
        }

        taps(w, lw, k as f32, &mut self.x_taps);
        taps(h, lh, k as f32, &mut self.y_taps);
        let masked = state.is_masked();
        let low = &self.low;
        let row = &mut self.row;
        row.resize(lw, [0.0; 3]);
        for (oy, row_out) in out.chunks_exact_mut(w * 4).enumerate() {
            // Vertical blend of the two source rows, then horizontal blend per output pixel.
            let (ya, yb, ty) = self.y_taps[oy];
            let (src_a, src_b) = (&low[ya * lw..(ya + 1) * lw], &low[yb * lw..(yb + 1) * lw]);
            for ((dst, a), b) in row.iter_mut().zip(src_a).zip(src_b) {
                *dst = std::array::from_fn(|c| (1.0 - ty) * f32::from(a[c]) + ty * f32::from(b[c]));
            }
            for (ox, (px_out, &(xa, xb, tx))) in
                row_out.chunks_exact_mut(4).zip(&self.x_taps).enumerate()
            {
                if masked && !state.covers(ox as u32, oy as u32) {
                    px_out.copy_from_slice(&[0, 0, 0, 255]);
                    continue;
                }
                let (pa, pb) = (row[xa], row[xb]);
                let rgb: [u8; 3] =
                    std::array::from_fn(|c| ((1.0 - tx) * pa[c] + tx * pb[c] + 0.5) as u8);
                px_out.copy_from_slice(&[rgb[order[0]], rgb[order[1]], rgb[order[2]], 255]);
            }
        }
    }
}

/// Splats one ribbon into the alpha/core buffers and multiplies its screen contribution into
/// the transmission buffer. Only the bounding box of the strip is cleared and composited.
#[allow(clippy::too_many_arguments)]
fn splat_stream(
    alpha: &mut [f32],
    core: &mut [f32],
    transmit: &mut [[f32; 3]],
    pts: &[StripPoint],
    glow: &[(f32, f32)],
    (lw, lh): (usize, usize),
    inv: f32,
    col: [f32; 3],
) {
    let (mut x0, mut y0, mut x1, mut y1, mut wmax) =
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN, 0.0f32);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
        wmax = wmax.max(p.w);
    }
    let reach = GLOW_Q_MAX.sqrt() * wmax * inv;
    let rx0 = (x0 * inv - reach).floor().max(0.0) as usize;
    let ry0 = (y0 * inv - reach).floor().max(0.0) as usize;
    let rx1 = ((x1 * inv + reach).ceil().max(0.0) as usize).min(lw);
    let ry1 = ((y1 * inv + reach).ceil().max(0.0) as usize).min(lh);
    if rx0 >= rx1 || ry0 >= ry1 {
        return;
    }
    for y in ry0..ry1 {
        let row = y * lw;
        alpha[row + rx0..row + rx1].fill(0.0);
        core[row + rx0..row + rx1].fill(0.0);
    }
    let region = (rx0, ry0, rx1, ry1);
    for seg in pts.windows(2) {
        splat(alpha, core, lw, region, glow, seg[0], seg[1], inv);
    }
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * lw + x;
            let a = alpha[i];
            if a <= 0.0 {
                continue;
            }
            let mix = WHITE_MIX * core[i];
            for c in 0..3 {
                let colour = col[c] + (1.0 - col[c]) * mix;
                transmit[i][c] *= 1.0 - a * colour;
            }
        }
    }
}

/// Analytic capsule splat for one centreline segment with linearly varying width and coverage.
///
/// Only pixels whose row span lies inside the capsule of radius `reach` are visited; every other
/// pixel fails the `d2 >= GLOW_Q_MAX * w2` cut-off anyway, so the output is unchanged.
#[allow(clippy::too_many_arguments)]
fn splat(
    alpha: &mut [f32],
    core: &mut [f32],
    lw: usize,
    (rx0, ry0, rx1, ry1): (usize, usize, usize, usize),
    glow: &[(f32, f32)],
    a: StripPoint,
    b: StripPoint,
    inv: f32,
) {
    let (ax, ay, bx, by) = (a.x * inv, a.y * inv, b.x * inv, b.y * inv);
    let wa = (a.w * inv).max(1e-4);
    let wb = (b.w * inv).max(1e-4);
    let (ex, ey) = (bx - ax, by - ay);
    let len2 = ex * ex + ey * ey;
    let inv_len2 = if len2 > 1e-12 { 1.0 / len2 } else { 0.0 };
    let reach = GLOW_Q_MAX.sqrt() * wa.max(wb);
    let x0 = ((ax.min(bx) - reach).floor().max(rx0 as f32)) as usize;
    let y0 = ((ay.min(by) - reach).floor().max(ry0 as f32)) as usize;
    let x1 = ((ax.max(bx) + reach).ceil().min(rx1 as f32)).max(0.0) as usize;
    let y1 = ((ay.max(by) + reach).ceil().min(ry1 as f32)).max(0.0) as usize;
    // Margin keeps the f64 span safely outside any pixel that f32 would accept.
    let r = f64::from(reach) * (1.0 + 1e-3) + 1e-3;
    let (a64, e64, len2_64) = (
        [f64::from(ax), f64::from(ay)],
        [f64::from(ex), f64::from(ey)],
        f64::from(len2),
    );
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let (lo, hi) = capsule_row_span(a64, e64, len2_64, r, f64::from(py));
        // Empty rows (`lo > hi`) and NaN spans fall back to the bounding box, never past it.
        let xs = ((lo - 0.5).floor() - 1.0).max(x0 as f64);
        let xe = ((hi - 0.5).ceil() + 2.0).min(x1 as f64);
        if !(xs < xe) {
            continue;
        }
        let (xs, xe) = (xs as usize, xe as usize);
        let mut x = xs;
        // Lane blocks: the geometry is pure per-lane f32 arithmetic (auto-vectorised, identical
        // IEEE results to a scalar loop); only accepted lanes touch the LUT and the buffers.
        while x + LANES <= xe {
            let mut inside = [false; LANES];
            let (mut q, mut tt) = ([0f32; LANES], [0f32; LANES]);
            for l in 0..LANES {
                let px = (x + l) as f32 + 0.5;
                let t = (((px - ax) * ex + (py - ay) * ey) * inv_len2).clamp(0.0, 1.0);
                tt[l] = t;
                let dx = px - (ax + t * ex);
                let dy = py - (ay + t * ey);
                let w = wa + t * (wb - wa);
                let (w2, d2) = (w * w, dx * dx + dy * dy);
                inside[l] = d2 < GLOW_Q_MAX * w2;
                q[l] = d2 / w2;
            }
            for l in (0..LANES).filter(|&l| inside[l]) {
                let t = tt[l];
                let (g, c) = glow[((q[l] * GLOW_LUT_SCALE) as usize).min(GLOW_LUT_LEN - 1)];
                let v = ((a.a + t * (b.a - a.a)) * g).min(1.0);
                let i = y * lw + x + l;
                if v > alpha[i] {
                    alpha[i] = v;
                    core[i] = c;
                }
            }
            x += LANES;
        }
        for x in x..xe {
            let px = x as f32 + 0.5;
            let t = (((px - ax) * ex + (py - ay) * ey) * inv_len2).clamp(0.0, 1.0);
            let dx = px - (ax + t * ex);
            let dy = py - (ay + t * ey);
            let w = wa + t * (wb - wa);
            let (w2, d2) = (w * w, dx * dx + dy * dy);
            if d2 >= GLOW_Q_MAX * w2 {
                continue;
            }
            let q = d2 / w2;
            let (g, c) = glow[((q * GLOW_LUT_SCALE) as usize).min(GLOW_LUT_LEN - 1)];
            let v = ((a.a + t * (b.a - a.a)) * g).min(1.0);
            let i = y * lw + x;
            if v > alpha[i] {
                alpha[i] = v;
                core[i] = c;
            }
        }
    }
}

/// Hull `[lo, hi]` of the x-values where the capsule of radius `r` around `a` to `a + e` meets the
/// row `py`; `lo > hi` when the row misses it. The capsule is convex, so this is its exact row
/// extent. The pieces are two end discs and the band `|n.(P-a)| <= r` clipped to `0 <= e.(P-a) <= |e|^2`.
fn capsule_row_span(a: [f64; 2], e: [f64; 2], len2: f64, r: f64, py: f64) -> (f64, f64) {
    let mut span = (f64::INFINITY, f64::NEG_INFINITY);
    let mut hull = |lo: f64, hi: f64| span = (span.0.min(lo), span.1.max(hi));
    for c in [a, [a[0] + e[0], a[1] + e[1]]] {
        let dy = py - c[1];
        if dy.abs() <= r {
            let h = (r * r - dy * dy).sqrt();
            hull(c[0] - h, c[0] + h);
        }
    }
    if len2 > 1e-12 {
        let len = len2.sqrt();
        let (nx, ny) = (-e[1] / len, e[0] / len);
        let dy = py - a[1];
        let (l1, h1) = lin_range(nx, ny * dy, -r, r);
        let (l2, h2) = lin_range(e[0], e[1] * dy, 0.0, len2);
        let (lo, hi) = (l1.max(l2), h1.min(h2));
        if lo <= hi {
            hull(a[0] + lo, a[0] + hi);
        }
    }
    span
}

/// Offsets `d` with `lo <= coef * d + k <= hi`, as `(min, max)`; `lo > hi` when none.
fn lin_range(coef: f64, k: f64, lo: f64, hi: f64) -> (f64, f64) {
    if coef.abs() < 1e-12 {
        return if (lo..=hi).contains(&k) {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (f64::INFINITY, f64::NEG_INFINITY)
        };
    }
    let (p, q) = ((lo - k) / coef, (hi - k) / coef);
    (p.min(q), p.max(q))
}

/// Bilinear taps mapping each output index to a pair of source indices and a weight.
fn taps(out_len: usize, src_len: usize, factor: f32, dst: &mut Vec<(usize, usize, f32)>) {
    dst.clear();
    let max = (src_len - 1) as f32;
    for o in 0..out_len {
        let f = ((o as f32 + 0.5) / factor - 0.5).clamp(0.0, max);
        let i0 = f.floor() as usize;
        let i1 = (i0 + 1).min(src_len - 1);
        dst.push((i0, i1, f - i0 as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn no_ribbon_history_renders_pure_black() {
        let mut state = Ribbons::new(Config::default(), 0.0);
        state.clear_histories();
        let mut out = vec![9u8; 1280 * 720 * 4];
        Rasterizer::new(Quality::Half).render(&state, &mut out);
        assert!(
            out.chunks_exact(4)
                .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0 && p[3] == 255)
        );
    }

    #[test]
    fn rgba_render_is_bgra_render_with_swapped_channels() {
        let rect = crate::config::Rect {
            x: 0,
            y: 0,
            w: 640,
            h: 360,
        };
        for quality in [Quality::Half, Quality::Full] {
            for seed in [21u64, 4, 13] {
                let config = Config {
                    num: 6,
                    width_px: 11.0,
                    span_rects: vec![rect],
                    seed,
                };
                let mut state = Ribbons::new_with_speed_factor(config, 0.0, 1.0);
                let mut rast = Rasterizer::new(quality);
                let mut frame = 1u64;
                for t in [1u64, 3, 8] {
                    while frame <= t * 60 {
                        state.step_to(frame as f64 / 60.0);
                        frame += 1;
                    }
                    let mut bgra = vec![0u8; 640 * 360 * 4];
                    let mut rgba = vec![0u8; 640 * 360 * 4];
                    rast.render(&state, &mut bgra);
                    rast.render_rgba(&state, &mut rgba);
                    for px in bgra.chunks_exact_mut(4) {
                        px.swap(0, 2);
                    }
                    assert!(bgra == rgba, "quality {quality:?} seed {seed} t {t}s");
                }
            }
        }
    }

    #[test]
    fn taps_identity_at_full_quality_and_stay_in_range() {
        let mut t = Vec::new();
        taps(10, 10, 1.0, &mut t);
        assert!(
            t.iter()
                .enumerate()
                .all(|(i, &(a, b, w))| a == i && w == 0.0 && b == (i + 1).min(9))
        );
        taps(7, 4, 2.0, &mut t);
        assert!(
            t.iter()
                .all(|&(a, b, w)| a < 4 && b < 4 && (0.0..=1.0).contains(&w))
        );
    }

    #[test]
    fn glow_lut_matches_direct_profile() {
        let r = Rasterizer::new(Quality::Full);
        for &q in &[0.0f64, 0.5, 2.0, 10.0, 20.0] {
            let idx = ((q as f32) * GLOW_LUT_SCALE).round() as usize;
            let (g_lut, _) = r.glow[idx];
            let (g, _) = glow_terms(f64::from(idx as f32 / GLOW_LUT_SCALE));
            assert!((f64::from(g_lut) - g).abs() < 1e-5);
        }
    }

    #[test]
    fn quality_factors() {
        assert_eq!(Quality::Full.factor(), 1);
        assert_eq!(Quality::Half.factor(), 2);
        assert_eq!(Quality::Quarter.factor(), 4);
        assert_eq!(Quality::default(), Quality::Half);
    }
}
