#[cfg(test)]
mod cases {
    use super::super::*;

    /// Horizontal box pass over each row of `buf` (`w` px wide), in place. Each row is
    /// copied to `line` first so the sliding window reads unmodified inputs.
    fn box_horizontal_ref(buf: &mut [f32], line: &mut Vec<f32>, w: usize, r: usize) {
        let stride = w * 4;
        let pad = r * 4;
        // `line` holds the row edge-padded by `r` on both sides, so the window of output `x` is the
        // contiguous span `[x, x + 2r]` and needs no clamping. The sums use the same order as the
        // clamped form, so the output is bit-identical.
        // One extra tail pixel: the final slide reads one past the last window's `add` slot.
        fit_f32(line, stride + 2 * pad + 4);
        let inv = 1.0 / (2 * r + 1) as f32;
        for row in buf.chunks_exact_mut(stride) {
            let (head, rest) = line.split_at_mut(pad);
            let (body, tail) = rest.split_at_mut(stride);
            body.copy_from_slice(row);
            for chunk in head.chunks_exact_mut(4) {
                chunk.copy_from_slice(&row[..4]);
            }
            for chunk in tail.chunks_exact_mut(4) {
                chunk.copy_from_slice(&row[stride - 4..]);
            }
            let mut sum = [0.0f32; 4];
            for window in line[..(2 * r + 1) * 4].chunks_exact(4) {
                for c in 0..4 {
                    sum[c] += window[c];
                }
            }
            for (x, out) in row.chunks_exact_mut(4).enumerate() {
                for c in 0..4 {
                    out[c] = sum[c] * inv;
                }
                let add = (x + 2 * r + 1) * 4;
                let sub = x * 4;
                for c in 0..4 {
                    sum[c] += line[add + c] - line[sub + c];
                }
            }
        }
    }

    #[test]
    fn lanes_box_horizontal_is_bit_identical_to_reference() {
        let mut seed = 0x0bad_cafe_u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for w in [1usize, 2, 5, 17, 64, 129] {
            for r in [1usize, 2, 3, 5, 9] {
                let buf: Vec<f32> = (0..w * 3 * 4)
                    .map(|_| (rnd() % 65536) as f32 / 256.0)
                    .collect();
                let (mut a, mut b) = (buf.clone(), buf);
                let (mut la, mut lb) = (Vec::new(), Vec::new());
                box_horizontal_ref(&mut a, &mut la, w, r);
                box_horizontal(&mut b, &mut lb, w, r);
                assert!(
                    a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
                    "w {w} r {r}"
                );
            }
        }
    }

    #[test]
    #[ignore = "timing; run with --release --include-ignored --nocapture"]
    fn horizontal_ab() {
        let (w, h) = (480usize, 270usize);
        let base: Vec<f32> = (0..w * h * 4).map(|i| (i * 17 % 255) as f32).collect();
        let mut line = Vec::new();
        let (mut best_ref, mut best_lanes) = (f64::MAX, f64::MAX);
        for _ in 0..7 {
            for name in ["ref", "lanes"] {
                let mut buf = base.clone();
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    if name == "ref" {
                        box_horizontal_ref(std::hint::black_box(&mut buf), &mut line, w, 5);
                    } else {
                        box_horizontal(std::hint::black_box(&mut buf), &mut line, w, 5);
                    }
                }
                let ns = t.elapsed().as_nanos() as f64 / (10.0 * (w * h) as f64);
                if name == "ref" {
                    best_ref = best_ref.min(ns);
                } else {
                    best_lanes = best_lanes.min(ns);
                }
            }
        }
        println!(
            "HABMIN ref {best_ref:.3} lanes {best_lanes:.3} ns/px ratio {:.3}",
            best_lanes / best_ref
        );
    }
}
