//! Canonical `FlameWM` surface material contract, renderer neutral.
//! Settings, Explorer, Terminal, Shell and WM decorations share these fill,
//! contour and alpha-blending rules. The renderer-specific `UiColor` mapping
//! lives in flamewm-ui-x11::surface_style.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Shared Surfaces fill for desktop menus: `surfaceColor` RGB with the `surfaceOpacity` alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceFill {
    pub rgb: u32,
    pub alpha: u8,
}

const DEFAULT_SURFACE_COLOR: u32 = crate::glass::DARK.tint_rgb;
const DEFAULT_SURFACE_OPACITY: u8 = crate::glass::DARK.tint_percent;
const DEFAULT_SURFACE_RIM_PERCENT: u8 = crate::glass::DARK.rim_percent;

/// Maps `surfaceColor` / `surfaceOpacity` to a fill. Same mapping as the shell
/// `settings_projection::surface_background`; invalid colours fall back to the default.
#[must_use]
pub fn surface_fill(snapshot: &SettingsSnapshot) -> SurfaceFill {
    // Menus, popups and Start take the GlassRegular resolver: alpha comes from glassOpacityPercent,
    // reduceTransparency makes it opaque. `surfaceColor` stays the user tint when glass is translucent.
    let appearance = crate::glass::appearance_from_settings(snapshot);
    let material = crate::glass::resolve(
        crate::glass::MaterialRole::GlassRegular,
        appearance,
        100,
        crate::glass::RendererCaps {
            backdrop_blur: true,
        },
    );
    let rgb = if appearance.reduce_transparency {
        material.tint_rgb
    } else {
        match snapshot.values.get("surfaceColor") {
            Some(SettingValue::Text(value)) => parse_rgb(value),
            _ => None,
        }
        .unwrap_or(material.tint_rgb)
    };
    SurfaceFill {
        rgb,
        alpha: ((u32::from(material.alpha_permille) * 255 + 500) / 1000) as u8,
    }
}

/// Taskbar/Dock fill from the resolved Dock material: Blur Only neutral, Inherit glass, or Custom
/// Tint. Reduce transparency yields the opaque Dock solid.
#[must_use]
pub fn dock_fill(snapshot: &SettingsSnapshot) -> SurfaceFill {
    // Inherit is the shared surface: same surfaceColor, glass alpha and reduce-transparency solid
    // as popups and widgets, so the taskbar cannot drift from them.
    if crate::glass::appearance_from_settings(snapshot).taskbar_mode
        == crate::glass::TaskbarMode::Inherit
    {
        return surface_fill(snapshot);
    }
    let material = crate::glass::resolve(
        crate::glass::MaterialRole::Dock,
        crate::glass::appearance_from_settings(snapshot),
        100,
        crate::glass::RendererCaps {
            backdrop_blur: true,
        },
    );
    SurfaceFill {
        rgb: material.tint_rgb,
        alpha: ((u32::from(material.alpha_permille) * 255 + 500) / 1000) as u8,
    }
}

/// Backdrop blur percent that menus and inheriting widgets use: the regular glass blur, 0 when
/// reduce transparency is on.
#[must_use]
pub fn surface_blur_percent(snapshot: &SettingsSnapshot) -> u8 {
    let material = crate::glass::resolve(
        crate::glass::MaterialRole::GlassRegular,
        crate::glass::appearance_from_settings(snapshot),
        100,
        crate::glass::RendererCaps {
            backdrop_blur: true,
        },
    );
    crate::glass::blur_percent_from_px(material.blur_px)
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
        _ => DEFAULT_SURFACE_RIM_PERCENT,
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
    fn shared_contour_is_the_dark_glass_hairline_by_default() {
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

/// Glass material tuning read from the `surface*Percent` keys. Defaults are the Dark preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlassMaterial {
    pub saturation_percent: u32,
    pub rim_fade_percent: u32,
    pub shadow_percent: u32,
    pub highlight_percent: u32,
}

/// Reads the glass material keys; missing or non-percent values use the Dark preset.
#[must_use]
pub fn glass_material(snapshot: &SettingsSnapshot) -> GlassMaterial {
    let read = |key: &str, default: u8| match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => u32::from(*value),
        _ => u32::from(default),
    };
    let dark = crate::glass::DARK;
    GlassMaterial {
        saturation_percent: read("surfaceSaturationPercent", dark.saturation_percent).min(200),
        rim_fade_percent: read("surfaceRimFadePercent", dark.rim_fade_percent).min(100),
        shadow_percent: read("surfaceShadowPercent", dark.shadow_percent).min(100),
        highlight_percent: read("surfaceHighlightPercent", dark.highlight_percent).min(100),
    }
}

/// Liquid Glass surface values for one painter: shadow reach and peak, rim fade, top gloss alpha.
/// The caller converts this to the render-core `SurfaceMaterial`; ui-core has no renderer dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceMaterialValues {
    pub shadow_spread: u8,
    pub shadow_peak: u8,
    pub rim_fade_percent: u8,
    pub gloss_alpha: u8,
}

/// Maps `surfaceRimFadePercent` and `surfaceHighlightPercent` to rim fade and gloss alpha.
/// Glass surfaces never cast a shadow: the shadow is always off (`shadow_spread == 0`,
/// `shadow_peak == 0`), whatever `surfaceShadowPercent` holds. Only window frames shadow.
#[must_use]
pub fn surface_material_values(snapshot: &SettingsSnapshot) -> SurfaceMaterialValues {
    let material = glass_material(snapshot);
    let scale = |percent: u32| (percent * 255 / 100) as u8;
    SurfaceMaterialValues {
        shadow_spread: 0,
        shadow_peak: 0,
        rim_fade_percent: material.rim_fade_percent.min(100) as u8,
        gloss_alpha: scale(material.highlight_percent),
    }
}

/// Rim alpha at row `y` of `span` rows: `top_alpha` at the top edge, easing linearly down to
/// `top_alpha * fade_percent / 100` at the bottom edge. Integer math, rounds to nearest.
#[must_use]
pub fn rim_alpha_at(top_alpha: u8, fade_percent: u32, y: u32, span: u32) -> u8 {
    if span <= 1 {
        return top_alpha;
    }
    let t = u64::from(y.min(span - 1)) * 1000 / u64::from(span - 1);
    let fade = u64::from(fade_percent.min(100)) * 10;
    // Weight in tenths of a percent: 1000 at the top, `fade` at the bottom.
    let weight = 1000 - (1000 - fade) * t / 1000;
    ((u64::from(top_alpha) * weight + 500) / 1000) as u8
}

/// Scales each channel's distance from its Rec.601 luma by `percent / 100`, clamped to 0..=255.
/// 100 returns the input unchanged.
#[must_use]
pub fn saturate_rgb(rgb: u32, percent: u32) -> u32 {
    let channels = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff];
    let luma = (channels[0] * 77 + channels[1] * 150 + channels[2] * 29) / 256;
    let mut out = 0_u32;
    for (index, channel) in channels.iter().enumerate() {
        let value = luma as i64 + (*channel as i64 - luma as i64) * i64::from(percent) / 100;
        let clamped = value.clamp(0, 255) as u32;
        out |= clamped << (16 - 8 * index as u32);
    }
    out
}

/// Soft drop-shadow alpha `distance` pixels outside a shape. Peak `peak` at the edge, falling
/// quadratically to 0 at `spread` pixels. Pure math; no per-frame pass is implied by it.
#[must_use]
pub fn shadow_alpha(distance: u32, spread: u32, peak: u8) -> u8 {
    if spread == 0 || distance >= spread {
        return 0;
    }
    let remaining = u64::from(spread - distance);
    let spread = u64::from(spread);
    ((u64::from(peak) * remaining * remaining + spread * spread / 2) / (spread * spread)) as u8
}

#[cfg(test)]
mod surface_material_value_tests {
    use super::*;

    #[test]
    fn glass_surfaces_never_cast_a_shadow() {
        let dark = surface_material_values(&SettingsSnapshot::new(1));
        assert_eq!((dark.shadow_spread, dark.shadow_peak), (0, 0));
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceShadowPercent".into(), SettingValue::Percent(100));
        snapshot
            .values
            .insert("surfaceHighlightPercent".into(), SettingValue::Percent(100));
        snapshot
            .values
            .insert("surfaceRimFadePercent".into(), SettingValue::Percent(100));
        let off = surface_material_values(&snapshot);
        assert_eq!((off.shadow_spread, off.shadow_peak), (0, 0));
        assert_eq!(off.gloss_alpha, 255);
        assert_eq!(off.rim_fade_percent, 100);
    }
}

#[cfg(test)]
mod glass_material_tests {
    use super::*;

    #[test]
    fn rim_fades_from_top_alpha_to_fade_fraction_at_bottom() {
        assert_eq!(rim_alpha_at(200, 6, 0, 100), 200);
        assert_eq!(rim_alpha_at(200, 6, 99, 100), 12);
        let mid = rim_alpha_at(200, 6, 50, 101);
        assert!(mid > 12 && mid < 200);
        assert_eq!(rim_alpha_at(200, 100, 99, 100), 200);
        assert_eq!(rim_alpha_at(200, 0, 99, 100), 0);
        assert_eq!(rim_alpha_at(77, 50, 0, 1), 77);
    }

    #[test]
    fn saturation_is_identity_at_100_and_clamps() {
        assert_eq!(saturate_rgb(0x0012_3456, 100), 0x0012_3456);
        assert_eq!(saturate_rgb(0x00ff_ffff, 200), 0x00ff_ffff);
        assert_eq!(saturate_rgb(0x0000_0000, 200), 0);
        // Grey has no chroma: any boost leaves it unchanged.
        assert_eq!(saturate_rgb(0x0080_8080, 165), 0x0080_8080);
        // Boosting a reddish tone moves red further from green and blue.
        let boosted = saturate_rgb(0x0040_2020, 165);
        assert!((boosted >> 16) & 0xff > 0x40);
        assert!((boosted >> 8) & 0xff < 0x20);
    }

    #[test]
    fn shadow_peaks_at_edge_and_is_zero_beyond_spread() {
        assert_eq!(shadow_alpha(0, 20, 100), 100);
        assert_eq!(shadow_alpha(20, 20, 100), 0);
        assert_eq!(shadow_alpha(5, 0, 100), 0);
        assert!(shadow_alpha(10, 20, 100) < shadow_alpha(5, 20, 100));
        assert_eq!(shadow_alpha(0, 20, 0), 0);
    }

    #[test]
    fn material_defaults_to_dark_and_reads_keys() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(glass_material(&snapshot).saturation_percent, 170);
        snapshot
            .values
            .insert("surfaceSaturationPercent".into(), SettingValue::Percent(100));
        snapshot
            .values
            .insert("surfaceShadowPercent".into(), SettingValue::Percent(250));
        let material = glass_material(&snapshot);
        assert_eq!(material.saturation_percent, 100);
        assert_eq!(material.shadow_percent, 100);
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
    fn inside(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
        if x < 0.0 || y < 0.0 || x >= w || y >= h {
            return false;
        }
        if r <= 0.0 {
            return true;
        }
        let cx = x.clamp(r, w - r);
        let cy = y.clamp(r, h - r);
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= r * r
    }
    if w == 0 || h == 0 || x >= w || y >= h {
        return 0;
    }
    // Only pixels within one pixel of the outside can contribute.
    let r = radius.min(w.min(h) / 2);
    let near_left = x <= r;
    let near_right = x.saturating_add(r).saturating_add(1) >= w;
    let near_top = y <= r;
    let near_bottom = y.saturating_add(r).saturating_add(1) >= h;
    if (!(near_left || near_right) || !(near_top || near_bottom))
        && x >= 1
        && y >= 1
        && x + 1 < w
        && y + 1 < h
    {
        return 0;
    }
    let outer_r = r as f32;
    let inner_r = outer_r - 1.0;
    let mut hits = 0_u32;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = x as f32 + (sx as f32 + 0.5) / 4.0;
            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
            if inside(px, py, w as f32, h as f32, outer_r)
                && (w <= 2
                    || h <= 2
                    || !inside(
                        px - 1.0,
                        py - 1.0,
                        (w - 2) as f32,
                        (h - 2) as f32,
                        inner_r.max(0.0),
                    ))
            {
                hits += 1;
            }
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

#[cfg(test)]
mod glass_settings_contract_tests {
    use super::*;

    fn with(pairs: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in pairs {
            snapshot.values.insert((*key).into(), value.clone());
        }
        snapshot
    }

    #[test]
    fn dock_fill_follows_taskbar_material_mode() {
        let blur_only = dock_fill(&with(&[(
            "taskbarMaterialMode",
            SettingValue::Text("blur-only".into()),
        )]));
        assert_eq!((blur_only.rgb, blur_only.alpha), (0x00ff_ffff, 20));
        let inherit = dock_fill(&with(&[(
            "taskbarMaterialMode",
            SettingValue::Text("inherit".into()),
        )]));
        assert_eq!((inherit.rgb, inherit.alpha), (crate::glass::DARK.tint_rgb, 31));
        let custom = dock_fill(&with(&[
            ("taskbarMaterialMode", SettingValue::Text("custom-tint".into())),
            ("taskbarCustomTint", SettingValue::Text("#5879ee".into())),
            ("taskbarOpacityPercent", SettingValue::Percent(50)),
        ]));
        assert_eq!((custom.rgb, custom.alpha), (0x0058_79ee, 128));
    }

    #[test]
    fn inherit_taskbar_fill_is_exactly_the_shared_surface_fill() {
        for reduce in [false, true] {
            let snapshot = with(&[
                ("reduceTransparency", SettingValue::Boolean(reduce)),
                ("surfaceColor", SettingValue::Text("#333333".into())),
                ("taskbarMaterialMode", SettingValue::Text("inherit".into())),
            ]);
            let dock = dock_fill(&snapshot);
            let surface = surface_fill(&snapshot);
            assert_eq!((dock.rgb, dock.alpha), (surface.rgb, surface.alpha), "reduce={reduce}");
        }
    }

    #[test]
    fn reduce_transparency_makes_every_fill_and_dock_opaque_without_blur() {
        let reduced = with(&[("reduceTransparency", SettingValue::Boolean(true))]);
        let menu = surface_fill(&reduced);
        assert_eq!((menu.rgb, menu.alpha), (0x0017_2135, 255));
        assert_eq!(surface_blur_percent(&reduced), 0);
        let dock = dock_fill(&reduced);
        assert_eq!((dock.rgb, dock.alpha), (0x0017_2135, 255));
        let custom_dock = dock_fill(&with(&[
            ("reduceTransparency", SettingValue::Boolean(true)),
            ("taskbarMaterialMode", SettingValue::Text("custom-tint".into())),
            ("taskbarCustomTint", SettingValue::Text("#5879ee".into())),
        ]));
        assert_eq!((custom_dock.rgb, custom_dock.alpha), (0x0058_79ee, 255));
    }

    #[test]
    fn menu_blur_is_regular_glass_blur_as_percent() {
        // Default glassBlurPx 28 over 40 px is 70%.
        assert_eq!(surface_blur_percent(&SettingsSnapshot::new(1)), 70);
    }

    #[test]
    fn legacy_surface_keys_seed_glass_only_when_glass_key_absent() {
        let legacy = with(&[("surfaceBlurPercent", SettingValue::Percent(50))]);
        assert_eq!(crate::glass::appearance_from_settings(&legacy).glass_blur_px, 20);
        let both = with(&[
            ("surfaceBlurPercent", SettingValue::Percent(50)),
            ("glassBlurPx", SettingValue::Percent(10)),
        ]);
        assert_eq!(crate::glass::appearance_from_settings(&both).glass_blur_px, 10);
    }
}
