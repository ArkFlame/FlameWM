//! Compositor colour filters: a typed 4x4 affine matrix per filter, applied to sRGB channels.
//!
//! Pure model, no Smithay. The final composite pass will feed `ColourMatrix` to a texture
//! program uniform (render hook pending, see `docs/backlog/remaining-audit.md`).
//!
//! Precedent: `.vendor/vela-main/compositor/src/color.c` (filter set: grayscale, invert,
//! protanopia, deuteranopia, tritanopia, applied as a per-pixel matrix in the output path).
//! Vela is GPL-3.0, so only the choice of filter set is taken; the coefficients are the published
//! Machado, Oliveira and Fernandes (2009) severity-1.0 values and are re-typed here, not copied.
//! Grayscale uses the Rec. 709 luma weights. Invert is `1 - c`, expressed as an affine row.

/// Row-major 4x4 matrix over the homogeneous colour `(r, g, b, 1)`.
pub type ColourMatrix = [[f32; 4]; 4];

const IDENTITY: ColourMatrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A user-selected output filter. `None` is the product default and draws no pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColourFilter {
    #[default]
    None,
    Grayscale,
    Invert,
    Protanopia,
    Deuteranopia,
    Tritanopia,
}

impl ColourFilter {
    /// Parses the `colourFilter` setting value. Unknown text keeps the default, so a hand-edited
    /// file cannot leave the compositor in an undefined filter state.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "grayscale" => Self::Grayscale,
            "invert" => Self::Invert,
            "protanopia" => Self::Protanopia,
            "deuteranopia" => Self::Deuteranopia,
            "tritanopia" => Self::Tritanopia,
            _ => Self::None,
        }
    }

    /// The matrix for this filter, or `None` when no post-process pass is needed.
    pub const fn matrix(self) -> Option<ColourMatrix> {
        match self {
            Self::None => None,
            Self::Grayscale => Some([
                [0.2126, 0.7152, 0.0722, 0.0],
                [0.2126, 0.7152, 0.0722, 0.0],
                [0.2126, 0.7152, 0.0722, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            Self::Invert => Some([
                [-1.0, 0.0, 0.0, 1.0],
                [0.0, -1.0, 0.0, 1.0],
                [0.0, 0.0, -1.0, 1.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            Self::Protanopia => Some([
                [0.152_286, 1.052_583, -0.204_868, 0.0],
                [0.114_503, 0.786_281, 0.099_216, 0.0],
                [-0.003_882, -0.048_116, 1.051_998, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            Self::Deuteranopia => Some([
                [0.367_322, 0.860_646, -0.227_968, 0.0],
                [0.280_085, 0.672_501, 0.047_413, 0.0],
                [-0.011_820, 0.042_940, 0.968_881, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            Self::Tritanopia => Some([
                [1.255_528, -0.076_749, -0.178_779, 0.0],
                [-0.078_411, 0.930_809, 0.147_602, 0.0],
                [0.004_733, 0.691_367, 0.303_900, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
        }
    }
}

/// Applies `matrix` to one sRGB colour and clamps each channel to `0.0..=1.0`.
pub fn apply(matrix: &ColourMatrix, [r, g, b]: [f32; 3]) -> [f32; 3] {
    let row = |m: &[f32; 4]| (m[0] * r + m[1] * g + m[2] * b + m[3]).clamp(0.0, 1.0);
    [row(&matrix[0]), row(&matrix[1]), row(&matrix[2])]
}

/// The matrix a pass should use: the identity for `None`, so a disabled filter is a no-op.
pub fn effective_matrix(filter: ColourFilter) -> ColourMatrix {
    filter.matrix().unwrap_or(IDENTITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ColourFilter; 5] = [
        ColourFilter::Grayscale,
        ColourFilter::Invert,
        ColourFilter::Protanopia,
        ColourFilter::Deuteranopia,
        ColourFilter::Tritanopia,
    ];

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn setting_parses_known_names_and_defaults_otherwise() {
        assert_eq!(ColourFilter::from_setting("invert"), ColourFilter::Invert);
        assert_eq!(
            ColourFilter::from_setting("tritanopia"),
            ColourFilter::Tritanopia
        );
        assert_eq!(ColourFilter::from_setting("none"), ColourFilter::None);
        assert_eq!(ColourFilter::from_setting("Invert"), ColourFilter::None);
        assert_eq!(ColourFilter::from_setting(""), ColourFilter::None);
    }

    #[test]
    fn none_has_no_pass_and_identity_effective_matrix() {
        assert_eq!(ColourFilter::None.matrix(), None);
        assert!(close(
            apply(&effective_matrix(ColourFilter::None), [0.2, 0.5, 0.9]),
            [0.2, 0.5, 0.9]
        ));
    }

    #[test]
    fn every_filter_has_an_affine_row_of_zeros_and_a_unit_homogeneous_corner() {
        for filter in ALL {
            let m = filter.matrix().expect("filter has a matrix");
            assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0], "{filter:?}");
        }
    }

    #[test]
    fn invert_maps_channels_to_their_complement() {
        let m = ColourFilter::Invert.matrix().unwrap();
        assert!(close(apply(&m, [0.0, 0.25, 1.0]), [1.0, 0.75, 0.0]));
    }

    #[test]
    fn grayscale_maps_every_channel_to_rec709_luma() {
        let m = ColourFilter::Grayscale.matrix().unwrap();
        let grey = apply(&m, [1.0, 0.0, 0.0]);
        assert!(close(grey, [0.2126, 0.2126, 0.2126]));
        assert!(close(apply(&m, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]));
    }

    #[test]
    fn grayscale_row_sums_are_one_so_white_and_black_are_fixed() {
        let m = ColourFilter::Grayscale.matrix().unwrap();
        for row in &m[..3] {
            assert!((row[0] + row[1] + row[2] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn colour_blindness_matrices_keep_neutral_grey_close_to_grey() {
        // Machado severity-1.0 rows sum to ~1, so a mid grey stays mid grey.
        for filter in [
            ColourFilter::Protanopia,
            ColourFilter::Deuteranopia,
            ColourFilter::Tritanopia,
        ] {
            let m = filter.matrix().unwrap();
            let out = apply(&m, [0.5, 0.5, 0.5]);
            assert!(
                out.iter().all(|c| (c - 0.5).abs() < 0.02),
                "{filter:?} -> {out:?}"
            );
        }
    }

    #[test]
    fn protanopia_and_deuteranopia_shift_pure_red_and_green_toward_each_other() {
        let red = [1.0, 0.0, 0.0];
        let p = apply(&ColourFilter::Protanopia.matrix().unwrap(), red);
        let d = apply(&ColourFilter::Deuteranopia.matrix().unwrap(), red);
        assert!(
            p[1] > 0.0 && d[1] > 0.0,
            "red loses its pure-channel identity: {p:?} {d:?}"
        );
        assert!(p != red && d != red);
    }

    #[test]
    fn output_is_always_clamped_to_unit_range() {
        for filter in ALL {
            let m = filter.matrix().unwrap();
            for input in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.0, 1.0, 0.0]] {
                let out = apply(&m, input);
                assert!(
                    out.iter().all(|c| (0.0..=1.0).contains(c)),
                    "{filter:?} {out:?}"
                );
            }
        }
    }
}
