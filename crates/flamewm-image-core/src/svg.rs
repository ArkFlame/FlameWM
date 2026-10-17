//! SVG decode via resvg/usvg + tiny-skia. Alpha is preserved.

use crate::{RgbaImage, SvgColorScheme, check_size_limits, unpremultiply_swapped_to_rgba};

/// Returns true when the SVG references a themeable color scheme.
#[must_use]
pub fn uses_current_color_scheme(bytes: &[u8]) -> bool {
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
    contains(bytes, b"ColorScheme-")
        || contains(bytes, b"currentColor")
        || contains(bytes, b"current-color-scheme")
}

fn finite_size(tree: &resvg::usvg::Tree) -> Result<(f32, f32), String> {
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err("SVG dimensions must be finite and non-zero".to_string());
    }
    Ok((width, height))
}

/// Returns natural (intrinsic) SVG dimensions rounded up to whole pixels.
pub fn natural_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    let tree = resvg::usvg::Tree::from_data_nested(bytes, &resvg::usvg::Options::default())
        .map_err(|error| error.to_string())?;
    let (width, height) = finite_size(&tree)?;
    Ok((width.ceil() as u32, height.ceil() as u32))
}

/// Renders SVG to an exact target size, preserving alpha.
pub fn render(bytes: &[u8], width: u32, height: u32) -> Result<RgbaImage, String> {
    render_inner(bytes, width, height, None)
}

/// Renders SVG with a semantic color scheme overriding `ColorScheme-*`.
pub fn render_with_color_scheme(
    bytes: &[u8],
    width: u32,
    height: u32,
    scheme: &SvgColorScheme,
) -> Result<RgbaImage, String> {
    render_inner(bytes, width, height, Some(scheme))
}

fn rgba_css(color: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn sheet_css(scheme: &SvgColorScheme) -> String {
    let text = rgba_css(scheme.text);
    let background = rgba_css(scheme.background);
    let highlight = rgba_css(scheme.highlight);
    let negative = rgba_css(scheme.negative_text);
    let classes = [
        "ColorScheme-Text",
        "ColorScheme-ViewText",
        "ColorScheme-ButtonText",
        "ColorScheme-Background",
        "ColorScheme-ViewBackground",
        "ColorScheme-ButtonBackground",
        "ColorScheme-Highlight",
        "ColorScheme-Accent",
        "ColorScheme-ViewHover",
        "ColorScheme-ViewFocus",
        "ColorScheme-ButtonHover",
        "ColorScheme-ButtonFocus",
        "ColorScheme-NegativeText",
    ];
    let mut css = String::new();
    for class in classes {
        let color = if class.ends_with("Text") {
            if class == "ColorScheme-NegativeText" {
                &negative
            } else {
                &text
            }
        } else if class.contains("Background") {
            &background
        } else {
            &highlight
        };
        css.push_str(&format!(".{class} {{ color: {color} !important; }}\n"));
    }
    css
}

fn render_inner(
    bytes: &[u8],
    width: u32,
    height: u32,
    scheme: Option<&SvgColorScheme>,
) -> Result<RgbaImage, String> {
    check_size_limits(width, height)?;
    let mut options = resvg::usvg::Options::default();
    if let Some(scheme) = scheme {
        options.style_sheet = Some(sheet_css(scheme));
    }
    let tree = if scheme.is_some() {
        // `from_data_nested` rebuilds `Options` internally and drops
        // `style_sheet`; themed renders must use `from_data`.
        resvg::usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?
    } else {
        resvg::usvg::Tree::from_data_nested(bytes, &options).map_err(|error| error.to_string())?
    };
    let (svg_width, svg_height) = finite_size(&tree)?;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).ok_or("requested dimensions overflow")?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / svg_width,
        height as f32 / svg_height,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    if pixmap.width() != width || pixmap.height() != height {
        return Err("raster dimensions do not match request".to_string());
    }
    let pixels = unpremultiply_swapped_to_rgba(pixmap.data());
    RgbaImage::from_rgba8(width, height, pixels)
        .ok_or_else(|| "requested dimensions overflow".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "../../assets/web/flamewm-icon.svg";
    const WIRELESS: &str = "../../assets/web/breeze/network-wireless.svg";
    const FOLDER: &str = "../../assets/web/breeze/folder.svg";
    const GAMES: &str = "../../assets/web/breeze/applications-games.svg";

    fn fixture(path: &str) -> Vec<u8> {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        std::fs::read(full).expect("SVG fixture")
    }

    fn alpha_coverage(image: &RgbaImage) -> u32 {
        image.pixels.chunks_exact(4).filter(|pixel| pixel[3] > 8).count() as u32
    }

    fn mean_rgb(image: &RgbaImage) -> (f32, f32, f32, u32) {
        let mut sum = [0u64; 4];
        let mut count = 0u32;
        for pixel in image.pixels.chunks_exact(4) {
            if pixel[3] > 8 {
                sum[0] += u64::from(pixel[0]);
                sum[1] += u64::from(pixel[1]);
                sum[2] += u64::from(pixel[2]);
                sum[3] += u64::from(pixel[3]);
                count += 1;
            }
        }
        assert!(count > 0, "expected visible pixels");
        (
            sum[0] as f32 / count as f32,
            sum[1] as f32 / count as f32,
            sum[2] as f32 / count as f32,
            count,
        )
    }

    #[test]
    fn renders_fixture_with_alpha() {
        let bytes = fixture(FIXTURE);
        let image = render(&bytes, 8, 8).unwrap();
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(image.pixels.len(), 8 * 8 * 4);
        assert!(
            image
                .pixels
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn rejects_invalid_svg() {
        assert!(render(b"not svg", 8, 8).is_err());
    }

    #[test]
    fn wireless_semantic_text_maps_to_flame_default() {
        let bytes = fixture(WIRELESS);
        assert!(uses_current_color_scheme(&bytes));
        let scheme = SvgColorScheme::flame_default();
        let image = render_with_color_scheme(&bytes, 22, 22, &scheme).unwrap();
        let (r, g, b, _) = mean_rgb(&image);
        assert!(
            (r - 0xf1 as f32).abs() < 20.0
                && (g - 0xf2 as f32).abs() < 20.0
                && (b - 0xf3 as f32).abs() < 20.0,
            "mean=({r},{g},{b}) expected near #f1f2f3"
        );
    }

    #[test]
    fn folder_accent_maps_to_highlight_preserving_alpha() {
        let bytes = fixture(FOLDER);
        assert!(uses_current_color_scheme(&bytes));
        let scheme = SvgColorScheme::flame_default();
        let plain = render(&bytes, 48, 48).unwrap();
        let themed = render_with_color_scheme(&bytes, 48, 48, &scheme).unwrap();
        let themed_alpha = alpha_coverage(&themed);
        let plain_alpha = alpha_coverage(&plain);
        assert!(themed_alpha > 0);
        // Alpha coverage (shape area) must be preserved by recoloring.
        assert!(
            (themed_alpha as i64 - plain_alpha as i64).abs() <= (plain_alpha as i64 / 10 + 8),
            "alpha coverage changed: plain={plain_alpha} themed={themed_alpha}"
        );
        let (r, g, b, _) = mean_rgb(&themed);
        // White/translucent overlay paths shift the global mean, so assert the
        // accent body directly: center pixel near #ef4048 and a large accent
        // population, mirrored by #3daee9 in the unthemed render.
        let center = |image: &RgbaImage| {
            let i = (30usize * 48 + 24) * 4;
            [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
        };
        let near = |pixel: [u8; 3], target: [u8; 3], tol: i16| {
            (i16::from(pixel[0]) - i16::from(target[0])).abs() <= tol
                && (i16::from(pixel[1]) - i16::from(target[1])).abs() <= tol
                && (i16::from(pixel[2]) - i16::from(target[2])).abs() <= tol
        };
        let count_near = |image: &RgbaImage, target: [u8; 3], tol: i16| {
            image
                .pixels
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 8 && near([pixel[0], pixel[1], pixel[2]], target, tol))
                .count()
        };
        assert!(
            near(center(&themed), [0xef, 0x40, 0x48], 24),
            "center={:?} expected near #ef4048 (mean=({r},{g},{b}))",
            center(&themed)
        );
        assert!(count_near(&themed, [0xef, 0x40, 0x48], 32) > 200);
        assert!(count_near(&plain, [0x3d, 0xae, 0xe9], 32) > 200);
    }

    #[test]
    fn full_color_icon_unaffected_by_scheme_signal() {
        let bytes = fixture(GAMES);
        assert!(!uses_current_color_scheme(&bytes));
        let plain = render(&bytes, 32, 32).unwrap();
        let themed =
            render_with_color_scheme(&bytes, 32, 32, &SvgColorScheme::flame_default()).unwrap();
        // No ColorScheme classes: injected class rules must not move pixels.
        assert_eq!(plain.pixels, themed.pixels);
    }
}
