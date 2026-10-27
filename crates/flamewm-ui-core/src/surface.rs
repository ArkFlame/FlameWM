//! Canonical `FlameWM` surface material contract, renderer neutral.
//! Settings, Explorer, Terminal, Shell and WM decorations share these fill,
//! contour and alpha-blending rules. The renderer-specific `UiColor` mapping
//! lives in `flamewm-ui-x11::surface_style`.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Shared Surfaces fill for desktop menus: `surfaceColor` RGB with the `surfaceOpacity` alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceFill {
    pub rgb: u32,
    pub alpha: u8,
}

const DEFAULT_SURFACE_COLOR: u32 = 0x0019_1b1d;
const DEFAULT_SURFACE_OPACITY: u8 = 50;

/// Maps `surfaceColor` / `surfaceOpacity` to a fill. Same mapping as the shell
/// `settings_projection::surface_background`; invalid colours fall back to the default.
#[must_use]
pub fn surface_fill(snapshot: &SettingsSnapshot) -> SurfaceFill {
    let rgb = match snapshot.values.get("surfaceColor") {
        Some(SettingValue::Text(value)) => parse_rgb(value),
        _ => None,
    }
    .unwrap_or(DEFAULT_SURFACE_COLOR);
    let percent = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => DEFAULT_SURFACE_OPACITY,
    };
    SurfaceFill {
        rgb,
        alpha: ((u16::from(percent) * 255) / 100) as u8,
    }
}

/// One pixel *inner* surface contour; independent of surface fill opacity.
/// All native surface renderers consume this contract instead of defining their own grey borders.
#[must_use]
pub fn surface_stroke(snapshot: &SettingsSnapshot) -> SurfaceFill {
    let rgb = match snapshot.values.get("surfaceBorderColor") {
        Some(SettingValue::Text(value)) => parse_rgb(value),
        _ => None,
    }
    .unwrap_or(0x00ff_ffff);
    let opacity = match snapshot.values.get("surfaceBorderOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 25,
    };
    SurfaceFill {
        rgb,
        alpha: ((u16::from(opacity) * 255) / 100) as u8,
    }
}

#[cfg(test)]
mod surface_stroke_contract_tests {
    use super::*;
    #[test]
    fn shared_contour_is_white_at_twenty_five_percent_by_default() {
        let stroke = surface_stroke(&SettingsSnapshot::new(1));
        assert_eq!(stroke.rgb, 0x00ff_ffff);
        assert_eq!(stroke.alpha, 63);
    }
    #[test]
    fn border_is_independent_from_surface_opacity_and_accepts_zero() {
        let mut s = SettingsSnapshot::new(1);
        s.values
            .insert("surfaceOpacity".into(), SettingValue::Percent(100));
        s.values.insert(
            "surfaceBorderColor".into(),
            SettingValue::Text("#112233".into()),
        );
        s.values
            .insert("surfaceBorderOpacity".into(), SettingValue::Percent(0));
        assert_eq!(
            surface_stroke(&s),
            SurfaceFill {
                rgb: 0x0011_2233,
                alpha: 0
            }
        );
        s.values
            .insert("surfaceBorderOpacity".into(), SettingValue::Percent(100));
        assert_eq!(surface_stroke(&s).alpha, 255);
    }
}

/// Blend a configured surface contour over an opaque RGB pixel. This is
/// shared by X11 frame pixels and software-rendered titlebar edges, which
/// must round channels identically at the configured opacity.
#[must_use]
pub const fn blend_surface_over_rgb(base: u32, overlay: SurfaceFill) -> u32 {
    let alpha = overlay.alpha as u32;
    let reverse = 255 - alpha;
    let source = overlay.rgb;
    let red = (((source >> 16 & 0xff) * alpha + (base >> 16 & 0xff) * reverse) / 255) << 16;
    let green = (((source >> 8 & 0xff) * alpha + (base >> 8 & 0xff) * reverse) / 255) << 8;
    let blue = ((source & 0xff) * alpha + (base & 0xff) * reverse) / 255;
    red | green | blue
}

#[cfg(test)]
mod surface_blend_tests {
    use super::*;
    #[test]
    fn blend_preserves_existing_channel_rounding() {
        for alpha in [0_u8, 1, 25, 63, 127, 254, 255] {
            for bg in [0, 0x0012_3456, 0x00ff_ffff, 0x0000_00ff] {
                for fg in [0, 0x00ab_cdef, 0x00ff_ffff] {
                    let expected = [16, 8, 0].into_iter().fold(0, |out, shift| {
                        out | (((((fg >> shift) & 255) * u32::from(alpha)
                            + ((bg >> shift) & 255) * (255 - u32::from(alpha)))
                            / 255)
                            << shift)
                    });
                    assert_eq!(
                        blend_surface_over_rgb(bg, SurfaceFill { rgb: fg, alpha }),
                        expected
                    );
                }
            }
        }
    }
}

/// Fractional coverage of a 1px inner outline on a rounded surface.
/// Both the native titlebar and UI renderer use the same surface material; this
/// pure geometry is reusable without coupling the skin to an X11 backend.
#[must_use]
pub fn inner_contour_coverage(w: u32, h: u32, radius: u32, x: u32, y: u32) -> u8 {
    if w == 0 || h == 0 || x >= w || y >= h {
        return 0;
    }
    // Only pixels within one pixel of the outside can contribute.
    let r = radius.min(w.min(h) / 2);
    let near_left = x <= r;
    let near_right = x.saturating_add(r).saturating_add(1) >= w;
    let near_top = y <= r;
    let near_bottom = y.saturating_add(r).saturating_add(1) >= h;
    if !(near_left || near_right) || !(near_top || near_bottom) {
        if x >= 1 && y >= 1 && x + 1 < w && y + 1 < h {
            return 0;
        }
    }
    fn inside(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
        if x < 0.0 || y < 0.0 || x >= w || y >= h { return false; }
        if r <= 0.0 { return true; }
        let cx = x.clamp(r, w - r);
        let cy = y.clamp(r, h - r);
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= r * r
    }
    let outer_r = r as f32;
    let inner_r = outer_r - 1.0;
    let mut hits = 0_u32;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = x as f32 + (sx as f32 + 0.5) / 4.0;
            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
            if inside(px, py, w as f32, h as f32, outer_r)
                && (w <= 2 || h <= 2 || !inside(px - 1.0, py - 1.0,
                    (w - 2) as f32, (h - 2) as f32, inner_r.max(0.0)))
            { hits += 1; }
        }
    }
    ((hits * 255 + 8) / 16) as u8
}

#[cfg(test)]
mod rounded_contour_tests {
    use super::inner_contour_coverage as alpha;
    #[test]
    fn outlined_corners_curve_and_interior_stays_empty() {
        assert_eq!(alpha(100, 60, 12, 0, 0), 0);
        assert!(alpha(100, 60, 12, 11, 0) > 0);
        assert!(alpha(100, 60, 12, 0, 11) > 0);
        assert_eq!(alpha(100, 60, 12, 30, 30), 0);
        assert!(alpha(100, 60, 12, 99, 49) > 0);
        assert_eq!(alpha(100, 60, 0, 30, 30), 0);
        assert_eq!(alpha(100, 60, 0, 0, 0), 255);
    }
}

/// Parses `#rrggbb` or bare `rrggbb` into 0xRRGGBB.
fn parse_rgb(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}
