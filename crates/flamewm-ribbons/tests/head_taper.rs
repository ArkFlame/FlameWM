use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};

const W: u32 = 1280;
const H: u32 = 720;
const WIDTH_PX: f64 = 11.0;
/// Fraction of the body peak that counts as "bright" for the area measure.
const BRIGHT_FRACTION: f64 = 0.6;

fn single_ribbon(seed: u64) -> Ribbons {
    let config = Config {
        num: 1,
        width_px: WIDTH_PX,
        span_rects: vec![Rect {
            x: 0,
            y: 0,
            w: W,
            h: H,
        }],
        seed,
    };
    Ribbons::new(config, 0.0)
}

/// Steps `state` to absolute time `t` seconds in 1/60 s frames.
fn advance_to(state: &mut Ribbons, t: f64) {
    let frames = (t * 60.0).round() as u64;
    for k in 1..=frames {
        state.step_to(k as f64 / 60.0);
    }
}

/// Per-pixel luminance (max of B, G, R) of a BGRA frame.
fn luminance(buf: &[u8], x: usize, y: usize) -> u8 {
    let p = &buf[(y * W as usize + x) * 4..][..3];
    p[0].max(p[1]).max(p[2])
}

/// Pixels inside the disk of radius `r` around `(cx, cy)`, as `(max luminance, count >= threshold)`.
/// The threshold is relative to `thresh_of_peak`, which is computed by the caller.
fn disk_stats(buf: &[u8], cx: f64, cy: f64, r: f64) -> (u8, Vec<u8>) {
    let mut vals = Vec::new();
    let mut peak = 0u8;
    let (x0, x1) = (
        (cx - r).floor().max(0.0) as usize,
        ((cx + r).ceil() as usize).min(W as usize - 1),
    );
    let (y0, y1) = (
        (cy - r).floor().max(0.0) as usize,
        ((cy + r).ceil() as usize).min(H as usize - 1),
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                let v = luminance(buf, x, y);
                peak = peak.max(v);
                vals.push(v);
            }
        }
    }
    (peak, vals)
}

/// Taper zone check: peak within 1 width of the newest point must be clearly below the trail-body
/// peak (3 widths around a point 6 widths behind). Also bounds the bright area around the head.
#[test]
fn head_has_no_bright_terminal_dot() {
    for &(seed, t) in &[
        (21u64, 1.0),
        (21, 2.0),
        (21, 5.0),
        (21, 8.0),
        (4, 2.0),
        (13, 5.0),
    ] {
        let mut r = single_ribbon(seed);
        advance_to(&mut r, t);
        let mut out = vec![0u8; (W * H * 4) as usize];
        Rasterizer::new(Quality::Full).render(&r, &mut out);
        let w = WIDTH_PX * r.scale();
        let head = r.streams()[0].position();
        let hist = r.streams()[0].history();
        let n = hist.len();
        let behind = (0..n)
            .rev()
            .map(|k| hist.get(k).pos)
            .find(|p| ((p[0] - head[0]).powi(2) + (p[1] - head[1]).powi(2)).sqrt() >= 6.0 * w)
            .unwrap_or_else(|| panic!("seed {seed} t {t}s: no trail point 6 widths behind head"));
        let (head_peak, _) = disk_stats(&out, head[0], head[1], w);
        let (_, head_vals) = disk_stats(&out, head[0], head[1], 3.0 * w);
        let (body_peak, body_vals) = disk_stats(&out, behind[0], behind[1], 3.0 * w);
        assert!(
            body_peak > 0,
            "seed {seed} t {t}s: trail body must be visible"
        );
        let cut = (f64::from(body_peak) * BRIGHT_FRACTION) as u8;
        let head_bright = head_vals.iter().filter(|&&v| v >= cut).count();
        let body_bright = body_vals.iter().filter(|&&v| v >= cut).count();
        println!(
            "seed {seed} t {t}s: head_peak {head_peak} body_peak {body_peak} head_bright {head_bright} body_bright {body_bright}"
        );
        assert!(
            f64::from(head_peak) <= f64::from(body_peak) * 0.9,
            "seed {seed} t {t}s: head peak {head_peak} within 1 width vs body peak {body_peak} (terminal hotspot)"
        );
        assert!(
            head_bright as f64 <= body_bright as f64 * 0.8,
            "seed {seed} t {t}s: head bright area {head_bright} px vs body {body_bright} px (terminal dot)"
        );
    }
}
