use crate::rng::SplitMix64;

/// Base palette: white, yellow, cyan, green, magenta, dark red.
const PALETTE: [[u8; 3]; 6] = [
    [0xF4, 0xFF, 0xF5],
    [0xF3, 0xF0, 0x19],
    [0x29, 0xEE, 0xDC],
    [0x18, 0xE8, 0x52],
    [0xDF, 0x22, 0xEF],
    [0xB2, 0x0A, 0x2D],
];

/// Ribbon colours for `num` ribbons. N=6 uses the exact palette; other counts cycle it with a
/// seeded hue offset of at most 9 degrees, so N>=3 keeps the white (index 0) and cyan (index 2).
#[must_use]
pub fn palette(num: usize, seed: u64) -> Vec<[u8; 3]> {
    let mut rng = SplitMix64::new(seed ^ 0xD1B5_4A32_D192_ED03);
    (0..num)
        .map(|i| {
            let base = PALETTE[i % PALETTE.len()];
            if num == PALETTE.len() {
                base
            } else {
                hue_shift(base, rng.unit() * 18.0 - 9.0)
            }
        })
        .collect()
}

/// Rotates hue by `degrees` (HSV model).
pub fn hue_shift(rgb: [u8; 3], degrees: f64) -> [u8; 3] {
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let sat = if max == 0.0 { 0.0 } else { delta / max };
    let hue = (hue + degrees).rem_euclid(360.0);
    let c = max * sat;
    let x = c * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = max - c;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m].map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// sRGB byte to linear light.
pub fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear RGB of an sRGB colour.
pub fn to_linear(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(srgb_to_linear)
}

/// Lookup table for linear light to sRGB byte, indexed by `round(lin * (len - 1))`.
pub fn encode_table(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| {
            let l = i as f64 / (len - 1) as f64;
            let s = if l <= 0.003_130_8 {
                12.92 * l
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// Per-channel screen blend `1 - (1 - dst)(1 - alpha * c)`.
#[cfg(test)]
pub fn screen_blend(dst: f32, alpha: f32, c: f32) -> f32 {
    1.0 - (1.0 - dst) * (1.0 - alpha * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_blend_identities() {
        for &dst in &[0.0f32, 0.25, 0.7, 1.0] {
            assert_eq!(screen_blend(dst, 0.0, 0.9), dst, "alpha 0 keeps dst");
            for &c in &[0.0f32, 0.3, 1.0] {
                let out = screen_blend(dst, 1.0, c);
                assert!((out - (1.0 - (1.0 - dst) * (1.0 - c))).abs() < 1e-7);
                assert!(out >= dst - 1e-7 && out >= c - 1e-7 && out <= 1.0 + 1e-7);
            }
        }
        assert_eq!(screen_blend(0.0, 1.0, 0.6), 0.6, "black dst shows colour");
        assert_eq!(screen_blend(0.4, 1.0, 1.0), 1.0, "white saturates");
    }

    #[test]
    fn palette_has_six_exact_colours_and_cycles_otherwise() {
        assert_eq!(palette(6, 9), PALETTE.to_vec());
        let p = palette(12, 9);
        assert_eq!(p.len(), 12);
        assert_eq!(palette(3, 9).len(), 3);
        // Hue offset stays within 9 degrees, so cyan remains cyan-ish (green < blue ~ red dominance).
        let cyan = hue_shift(PALETTE[2], 9.0);
        assert!(cyan[1] > cyan[0] && cyan[2] > cyan[0]);
    }

    #[test]
    fn srgb_round_trip_is_close() {
        for c in (0..=255u8).step_by(5) {
            let lin = srgb_to_linear(c);
            let back = encode_table(4096)[(lin * 4095.0).round() as usize];
            assert!(
                (i32::from(back) - i32::from(c)).abs() <= 1,
                "c={c} back={back}"
            );
        }
    }
}
