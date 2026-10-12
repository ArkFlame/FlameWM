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
            *dst = t.map(|v| encode[((1.0 - v).clamp(0.0, 1.0) * (ENCODE_LEN - 1) as f32 + 0.5) as usize]);
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
            for (ox, px_out) in row_out.chunks_exact_mut(4).enumerate() {
                if masked && !state.covers(ox as u32, oy as u32) {
                    px_out.copy_from_slice(&[0, 0, 0, 255]);
                    continue;
                }
                let (xa, xb, tx) = self.x_taps[ox];
                let (pa, pb) = (row[xa], row[xb]);
                let rgb: [u8; 3] = std::array::from_fn(|c| ((1.0 - tx) * pa[c] + tx * pb[c] + 0.5) as u8);
                px_out.copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
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
    let (mut x0, mut y0, mut x1, mut y1, mut wmax) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN, 0.0f32);
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
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        for x in x0..x1 {
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
        assert!(out.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0 && p[3] == 255));
    }

    #[test]
    fn taps_identity_at_full_quality_and_stay_in_range() {
        let mut t = Vec::new();
        taps(10, 10, 1.0, &mut t);
        assert!(t.iter().enumerate().all(|(i, &(a, b, w))| a == i && w == 0.0 && b == (i + 1).min(9)));
        taps(7, 4, 2.0, &mut t);
        assert!(t.iter().all(|&(a, b, w)| a < 4 && b < 4 && (0.0..=1.0).contains(&w)));
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
