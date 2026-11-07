//! Gamma ramps for `wlr-gamma-control`, derived from the Night Shift intensity.
//!
//! The intensity slider (0..=100, owned by `flamewm_api::blue_light`) maps to a colour temperature,
//! and the temperature maps to per-channel gains. The gains only ever reduce a channel, so the
//! neutral setting (intensity 0) is exactly the identity ramp.

/// Colour temperature of an untouched display, in kelvin.
pub const NEUTRAL_KELVIN: f64 = 6500.0;

/// Colour temperature at full intensity, in kelvin.
pub const WARMEST_KELVIN: f64 = 3000.0;

/// One ramp: a 16-bit lookup table per colour channel, all of the same length.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GammaRamp {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

/// Colour temperature for a slider intensity. Values above 100 saturate at the warmest step.
#[must_use]
pub fn kelvin_for_intensity(intensity: u8) -> f64 {
    let fraction = f64::from(intensity.min(100)) / 100.0;
    NEUTRAL_KELVIN - (NEUTRAL_KELVIN - WARMEST_KELVIN) * fraction
}

/// Black-body RGB for a temperature (Tanner Helland's fit), each channel in `0.0..=255.0`.
fn black_body_rgb(kelvin: f64) -> [f64; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let green = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    [red, green, blue].map(|channel| channel.clamp(0.0, 255.0))
}

/// Per-channel multipliers in `0.0..=1.0` relative to the neutral white point.
#[must_use]
pub fn channel_gains(kelvin: f64) -> [f64; 3] {
    let neutral = black_body_rgb(NEUTRAL_KELVIN);
    let target = black_body_rgb(kelvin);
    let mut gains = [0.0; 3];
    for (index, gain) in gains.iter_mut().enumerate() {
        *gain = if neutral[index] > 0.0 {
            (target[index] / neutral[index]).min(1.0)
        } else {
            1.0
        };
    }
    gains
}

/// The ramp for `size` entries per channel at a slider intensity. `size == 0` gives empty tables.
#[must_use]
pub fn ramp_for_intensity(intensity: u8, size: usize) -> GammaRamp {
    let [red_gain, green_gain, blue_gain] = channel_gains(kelvin_for_intensity(intensity));
    let table = |gain: f64| -> Vec<u16> {
        (0..size)
            .map(|index| {
                let linear = if size == 1 {
                    f64::from(u16::MAX)
                } else {
                    index as f64 * f64::from(u16::MAX) / (size - 1) as f64
                };
                // Clamped to the u16 range, so the cast cannot truncate.
                (linear * gain).round().clamp(0.0, f64::from(u16::MAX)) as u16
            })
            .collect()
    };
    GammaRamp {
        red: table(red_gain),
        green: table(green_gain),
        blue: table(blue_gain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_intensity_is_the_identity_ramp() {
        let ramp = ramp_for_intensity(0, 256);
        let identity: Vec<u16> = (0..256).map(|index| (index * 65535 / 255) as u16).collect();
        assert_eq!(ramp.red, identity);
        assert_eq!(ramp.green, identity);
        assert_eq!(ramp.blue, identity);
    }

    #[test]
    fn intensity_maps_to_temperature_range() {
        assert_eq!(kelvin_for_intensity(0), NEUTRAL_KELVIN);
        assert_eq!(kelvin_for_intensity(100), WARMEST_KELVIN);
        assert_eq!(kelvin_for_intensity(255), WARMEST_KELVIN);
        assert!(kelvin_for_intensity(50) < NEUTRAL_KELVIN);
        assert!(kelvin_for_intensity(50) > WARMEST_KELVIN);
    }

    #[test]
    fn gains_only_reduce_and_neutral_is_unity() {
        assert_eq!(channel_gains(NEUTRAL_KELVIN), [1.0, 1.0, 1.0]);
        for gain in channel_gains(4000.0) {
            assert!((0.0..=1.0).contains(&gain));
        }
    }

    #[test]
    fn warm_intensity_keeps_red_and_cuts_blue_most() {
        let [red, green, blue] = channel_gains(WARMEST_KELVIN);
        assert_eq!(red, 1.0);
        assert!(green < 1.0 && green > blue);
        assert!(blue < 0.5, "blue gain {blue} should drop sharply at 3000K");
    }

    #[test]
    fn warm_ramp_top_entries_order_red_green_blue() {
        let ramp = ramp_for_intensity(100, 256);
        let (r, g, b) = (
            *ramp.red.last().unwrap(),
            *ramp.green.last().unwrap(),
            *ramp.blue.last().unwrap(),
        );
        assert_eq!(r, u16::MAX);
        assert!(b < g && g < r, "expected b<g<r, got r={r} g={g} b={b}");
    }

    #[test]
    fn each_channel_is_monotonic_non_decreasing() {
        let ramp = ramp_for_intensity(70, 64);
        for table in [&ramp.red, &ramp.green, &ramp.blue] {
            assert!(table.windows(2).all(|pair| pair[0] <= pair[1]));
        }
    }

    #[test]
    fn size_edges_are_handled() {
        let empty = ramp_for_intensity(60, 0);
        assert!(empty.red.is_empty() && empty.green.is_empty() && empty.blue.is_empty());

        let single = ramp_for_intensity(0, 1);
        assert_eq!(single.red, vec![u16::MAX]);
        assert_eq!(single.blue, vec![u16::MAX]);
    }
}
