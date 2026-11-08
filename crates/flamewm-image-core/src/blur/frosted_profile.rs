#[cfg(test)]
mod cases {
    use super::super::*;

    /// Same-process per-stage profile of `frosted_backdrop_with` at quarter-scale desktop size.
    #[test]
    #[ignore = "timing; run with --release --include-ignored --nocapture"]
    fn frosted_stage_profile() {
        let (cw, ch) = (480usize, 270usize);
        let radius = 4.8f32;
        let pad = (2.0 * radius).ceil() as usize;
        let (pw, ph) = (cw + 2 * pad, ch + 2 * pad);
        let src: Vec<u8> = (0..pw * ph * 4).map(|i| (i * 31 % 251) as u8).collect();
        let n = (cw * ch) as f64;
        let iters = 30;
        let mut scratch = BlurScratch::new();
        let mut time = |name: &str, f: &mut dyn FnMut(&mut BlurScratch)| {
            f(&mut scratch);
            let t = std::time::Instant::now();
            for _ in 0..iters {
                f(&mut scratch);
            }
            println!(
                "FROSTAB {name}: {:.3} ns/px (output px)",
                t.elapsed().as_nanos() as f64 / (f64::from(iters) * n)
            );
        };
        time("premultiply_grid", &mut |s| {
            fit_f32(&mut s.work, pw * ph * 4);
            for (i, px) in src.chunks_exact(4).enumerate() {
                premultiply_pixel(px, &mut s.work[i * 4..i * 4 + 4]);
            }
        });
        time("box_h_x3", &mut |s| {
            fit_f32(&mut s.work, pw * ph * 4);
            for (i, px) in src.chunks_exact(4).enumerate() {
                premultiply_pixel(px, &mut s.work[i * 4..i * 4 + 4]);
            }
            for r in box_radii(radius / 2.0) {
                if r > 0 {
                    box_horizontal(&mut s.work, &mut s.line, pw, r);
                }
            }
        });
        time("blur_all_passes", &mut |s| {
            fit_f32(&mut s.work, pw * ph * 4);
            for (i, px) in src.chunks_exact(4).enumerate() {
                premultiply_pixel(px, &mut s.work[i * 4..i * 4 + 4]);
            }
            blur_boxes(&mut s.work, &mut s.line, &mut s.band, pw, ph, radius / 2.0);
        });
        time("full_frosted_backdrop", &mut |s| {
            let img = RgbaImage {
                width: pw as u32,
                height: ph as u32,
                pixels: src.clone(),
            };
            let r = frosted_backdrop_with(
                s,
                &img,
                Rect {
                    x: pad as i32,
                    y: pad as i32,
                    width: cw as u32,
                    height: ch as u32,
                },
                radius,
                None,
            );
            std::hint::black_box(r);
        });
        time("emit_unpremultiply", &mut |s| {
            let mut out = vec![0u8; cw * ch * 4];
            blur_emit(
                s,
                1,
                pw,
                ph,
                Window {
                    x: pad,
                    y: pad,
                    w: cw,
                    h: ch,
                },
                &mut out,
            );
            std::hint::black_box(&out);
        });
    }
}
