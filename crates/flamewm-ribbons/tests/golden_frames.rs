//! Golden-frame pin: FNV-1a 64 over the full BGRA buffer at fixed seed, size and time.
//! Any pixel change (even 1 LSB) fails this test. Run with `--nocapture` to print the hashes.

use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};

const W: u32 = 640;
const H: u32 = 360;
const SEEDS: [u64; 3] = [21, 4, 13];
const TIMES_S: [u64; 3] = [1, 3, 8];
const QUALITIES: [Quality; 2] = [Quality::Half, Quality::Full];

/// Expected hashes, ordered by quality, then seed, then time.
const GOLDEN: [u64; 18] = [
    0x3ebd_e9df_cc6e_e231,
    0x638c_c5a3_2e95_dba7,
    0x7b0d_cf5e_1371_dacc,
    0x421c_02c1_892e_7485,
    0x5cb6_1e0f_722e_d551,
    0x16a6_a311_0251_8a27,
    0x8149_1ab1_9cc8_ffe2,
    0x9146_bbfc_f2ec_ab04,
    0x4b23_f3e0_f866_c334,
    0x0865_1d32_c299_890e,
    0x18cb_6f68_cf81_c095,
    0xb7ec_74be_2913_a97e,
    0x35a7_d3e7_f6bf_6b38,
    0x9e82_39ef_859e_71d6,
    0xf1ea_5b2d_f5a3_c06d,
    0x6808_abc4_f595_0c63,
    0xa3f8_cf27_52df_a8d8,
    0x06be_8bc7_d3b2_ef4d,
];

fn fnv1a64(buf: &[u8]) -> u64 {
    buf.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn span_config(seed: u64) -> Config {
    Config {
        num: 6,
        width_px: 11.0,
        span_rects: vec![Rect {
            x: 0,
            y: 0,
            w: W,
            h: H,
        }],
        seed,
    }
}

/// Renders one frame at `t_seconds` after stepping from 0 at 60 Hz.
fn frame_hash(mut state: Ribbons, quality: Quality, t_seconds: u64) -> u64 {
    let mut rast = Rasterizer::new(quality);
    for frame in 1..=t_seconds * 60 {
        state.step_to(frame as f64 / 60.0);
    }
    let mut buf = vec![0u8; (W * H * 4) as usize];
    rast.render(&state, &mut buf);
    fnv1a64(&buf)
}

/// Hashes built at an explicit speed factor. The pins were recorded at factor 1.0.
fn hashes_with_factor(factor: f64) -> Vec<u64> {
    let mut out = Vec::new();
    for &quality in &QUALITIES {
        for &seed in &SEEDS {
            let mut state = Ribbons::new_with_speed_factor(span_config(seed), 0.0, factor);
            let mut rast = Rasterizer::new(quality);
            let mut frame = 1u64;
            let mut buf = vec![0u8; (W * H * 4) as usize];
            for &t in &TIMES_S {
                let last = t * 60;
                while frame <= last {
                    state.step_to(frame as f64 / 60.0);
                    frame += 1;
                }
                rast.render(&state, &mut buf);
                out.push(fnv1a64(&buf));
            }
        }
    }
    out
}

fn hashes() -> Vec<u64> {
    hashes_with_factor(1.0)
}

#[test]
fn default_speed_differs_from_unit_factor_at_3s() {
    // Half quality, seed 21, t=3s: index 1 in the pinned order (quality, seed, time).
    let unit = frame_hash(
        Ribbons::new_with_speed_factor(span_config(21), 0.0, 1.0),
        Quality::Half,
        3,
    );
    let default = frame_hash(Ribbons::new(span_config(21), 0.0), Quality::Half, 3);
    assert_eq!(unit, GOLDEN[1], "factor 1.0 must reproduce the pinned hash");
    assert_ne!(
        default, unit,
        "default speed must differ from factor 1.0 at t=3s"
    );
}

#[test]
fn golden_frames_are_bit_identical() {
    let got = hashes();
    for (i, h) in got.iter().enumerate() {
        println!("golden[{i}] = 0x{h:016x}");
    }
    assert_eq!(got.as_slice(), GOLDEN.as_slice());
}
