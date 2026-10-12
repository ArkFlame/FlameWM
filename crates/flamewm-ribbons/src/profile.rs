/// Trail lifetime in seconds.
pub const LIFETIME_S: f64 = 8.5;

/// Hermite smoothstep, clamped to `[0, 1]` outside `[e0, e1]`.
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Trail fade `F(age)` with `q = age / LIFETIME_S`: 1 for `q <= 0.55`, then
/// `(1 - smoothstep(0.55, 1, q))^1.6`, which is 0 at and beyond `q = 1`.
pub fn fade(q: f64) -> f64 {
    if q <= 0.55 {
        1.0
    } else {
        (1.0 - smoothstep(0.55, 1.0, q)).powf(1.6)
    }
}

/// Per-sample coverage `A = F(q) * (0.25 + 0.75 z)`.
pub fn coverage(age_s: f64, z: f64) -> f64 {
    fade(age_s.max(0.0) / LIFETIME_S) * (0.25 + 0.75 * z)
}

/// Ribbon width in px: `clamp(RibbonWidth * S * (0.72 + 0.55 z) * (0.75 + 0.25 |cos beta|), S, 36 S)`.
pub fn ribbon_width(z: f64, beta: f64, scale: f64, width_px: f64) -> f64 {
    (width_px * scale * (0.72 + 0.55 * z) * (0.75 + 0.25 * beta.cos().abs()))
        .clamp(scale, 36.0 * scale)
}

/// Gaussian terms at `q = (d / w)^2`. Returns `(profile, core)`: the clamped three-term profile
/// `G` and the central narrow term in `[0, 1]` used for the white core mix.
pub fn glow_terms(q: f64) -> (f64, f64) {
    let core = (-q / (2.0 * 0.18 * 0.18)).exp();
    let total = 0.76 * core
        + 0.26 * (-q / (2.0 * 0.72 * 0.72)).exp()
        + 0.075 * (-q / (2.0 * 1.90 * 1.90)).exp();
    (total.clamp(0.0, 1.0), core)
}

/// Across-ribbon profile `G(d, w)`, clamped to `[0, 1]`.
#[cfg(test)]
pub fn profile(d: f64, w: f64) -> f64 {
    glow_terms((d / w).powi(2)).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_endpoints_and_monotone() {
        assert_eq!(fade(0.0), 1.0);
        assert_eq!(fade(0.55), 1.0);
        assert_eq!(fade(1.0), 0.0);
        assert_eq!(fade(2.0), 0.0);
        let mut prev = 1.0;
        for i in 0..=1000 {
            let f = fade(i as f64 / 1000.0);
            assert!((0.0..=1.0).contains(&f));
            assert!(f <= prev + 1e-12, "fade must not increase");
            prev = f;
        }
    }

    #[test]
    fn coverage_uses_depth_and_age() {
        assert!((coverage(0.0, 1.0) - 1.0).abs() < 1e-12);
        assert!((coverage(0.0, 0.0) - 0.25).abs() < 1e-12);
        assert_eq!(coverage(LIFETIME_S, 1.0), 0.0);
    }

    #[test]
    fn width_is_clamped_to_scaled_range() {
        for &(z, beta) in &[(0.0, 0.6), (1.0, 0.0), (0.5, 0.3)] {
            for &s in &[0.6, 1.0, 2.5] {
                let w = ribbon_width(z, beta, s, 28.0);
                assert!(w >= s - 1e-12 && w <= 36.0 * s + 1e-12);
            }
        }
        // Narrowest configured width at the smallest depth still respects the S floor.
        assert!(ribbon_width(0.0, 0.0, 1.0, 2.0) >= 1.0);
        assert_eq!(ribbon_width(0.0, 0.0, 1.0, 2.0), 2.0 * 0.72 * 1.0);
        assert!(ribbon_width(1.0, 0.0, 2.5, 28.0) <= 36.0 * 2.5);
    }

    #[test]
    fn gaussian_profile_is_bounded_and_peaks_on_centreline() {
        for i in 0..=400 {
            let d = i as f64 * 0.1;
            let g = profile(d, 3.0);
            assert!((0.0..=1.0).contains(&g), "profile out of range at d={d}");
        }
        assert_eq!(profile(0.0, 5.0), 1.0, "0.76+0.26+0.075 clamps to 1");
        let (_, core_at_0) = glow_terms(0.0);
        assert!((core_at_0 - 1.0).abs() < 1e-12);
        assert!(glow_terms(24.0).0 < 3e-3, "cut-off radius 4.9w is negligible");
    }

    #[test]
    fn smoothstep_endpoints() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-12);
    }
}
