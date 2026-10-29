//! Shared liquid-glass surface tokens (Tahoe glass). Every native surface reads these through
//! the `surface` contract; presets only write existing `surface*` and `glass*` settings keys, so each
//! option stays individually editable.
//!
//! Mapping: `glassOpacityPercent` mirrors `surfaceOpacity` (tint alpha), and `glassBlurPx` is the
//! `surfaceBlurPercent` value converted to px (`blur_percent / 100 * BLUR_MAX_PX`, rounded).
//! The Dark and Light presets therefore keep the glass and surface keys in agreement.
//!
//! Source of truth: the regular glass and clear glass tokens of the Tahoe design-lab mockup
//! (`styles.css`). Percent fields convert CSS values: blur px / 40 * 100, alpha * 100.
//! Dark: regular glass `rgba(234,244,255,.12)`, 28px blur, saturate 170%, rim `.17`, gloss `.14`,
//! shadow peak `.37`. Light: clear glass `rgba(255,255,255,.045)`, 13px blur, saturate 155%,
//! rim `.23`, gloss `.20`, shadow peak `.12`.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Tahoe corner radii (px at 1x). Consumers pick the one matching their surface.
pub const WINDOW_RADIUS: u16 = 24;
pub const POPOVER_RADIUS: u16 = 12;
pub const MENU_RADIUS: u16 = 12;
pub const BUTTON_RADIUS: u16 = 9;

/// Blur percent to radius scale: `blur_percent / 100 * BLUR_MAX_PX` at 1x.
pub const BLUR_MAX_PX: f32 = 40.0;

/// One liquid-glass option set. Field names mirror the settings keys they write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlassPreset {
    /// `surfaceColor` tint RGB.
    pub tint_rgb: u32,
    /// `surfaceOpacity` tint alpha percent.
    pub tint_percent: u8,
    /// `surfaceBlurPercent`.
    pub blur_percent: u8,
    /// `surfaceBorderColor` rim RGB.
    pub rim_rgb: u32,
    /// `surfaceBorderOpacity` rim alpha percent.
    pub rim_percent: u8,
    /// `surfaceSaturationPercent`: backdrop saturation, 100 is identity.
    pub saturation_percent: u8,
    /// `surfaceRimFadePercent`: bottom rim alpha as a percent of the top rim.
    pub rim_fade_percent: u8,
    /// `surfaceShadowPercent`: peak alpha of the drop shadow.
    pub shadow_percent: u8,
    /// `surfaceHighlightPercent`: alpha of the 1px inset top gloss.
    pub highlight_percent: u8,
    /// `glassOpacityPercent` (4..=42): `GlassRegular` tint alpha. Same value as `tint_percent`,
    /// because the surface tint and the glass regular tint come from one CSS token.
    pub glass_opacity_percent: u8,
    /// `glassBlurPx` (0..=54): `GlassRegular` blur in logical px. Derived from `blur_percent`
    /// as `blur_percent / 100 * BLUR_MAX_PX`, rounded: Dark 70% -> 28px, Light 33% -> 13px.
    pub glass_blur_px: u8,
}

pub const DARK: GlassPreset = GlassPreset {
    tint_rgb: 0x00ea_f4ff,
    tint_percent: 12,
    blur_percent: 70,
    rim_rgb: 0x00ff_ffff,
    rim_percent: 17,
    saturation_percent: 170,
    rim_fade_percent: 100,
    shadow_percent: 0,
    highlight_percent: 14,
    glass_opacity_percent: 12,
    glass_blur_px: 28,
};

pub const LIGHT: GlassPreset = GlassPreset {
    tint_rgb: 0x00ff_ffff,
    tint_percent: 5,
    blur_percent: 33,
    rim_rgb: 0x00ff_ffff,
    rim_percent: 23,
    saturation_percent: 155,
    rim_fade_percent: 100,
    shadow_percent: 0,
    highlight_percent: 20,
    glass_opacity_percent: 5,
    glass_blur_px: 13,
};

impl GlassPreset {
    /// Settings keys and values this preset writes, in one fixed order.
    #[must_use]
    pub fn settings_values(self) -> [(&'static str, SettingValue); 11] {
        [
            (
                "surfaceColor",
                SettingValue::Text(format!("#{:06x}", self.tint_rgb)),
            ),
            ("surfaceOpacity", SettingValue::Percent(self.tint_percent)),
            (
                "surfaceBlurPercent",
                SettingValue::Percent(self.blur_percent),
            ),
            (
                "surfaceBorderColor",
                SettingValue::Text(format!("#{:06x}", self.rim_rgb)),
            ),
            (
                "surfaceBorderOpacity",
                SettingValue::Percent(self.rim_percent),
            ),
            (
                "surfaceSaturationPercent",
                SettingValue::Percent(self.saturation_percent),
            ),
            (
                "surfaceRimFadePercent",
                SettingValue::Percent(self.rim_fade_percent),
            ),
            (
                "surfaceShadowPercent",
                SettingValue::Percent(self.shadow_percent),
            ),
            (
                "surfaceHighlightPercent",
                SettingValue::Percent(self.highlight_percent),
            ),
            (
                "glassOpacityPercent",
                SettingValue::Percent(self.glass_opacity_percent),
            ),
            ("glassBlurPx", SettingValue::Percent(self.glass_blur_px)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_the_tahoe_default_and_light_is_clear() {
        assert_eq!(DARK.tint_rgb, 0x00ea_f4ff);
        assert_eq!(DARK.blur_percent, 70);
        assert!(LIGHT.tint_percent < DARK.tint_percent);
        assert!(LIGHT.rim_percent > DARK.rim_percent);
    }

    #[test]
    fn preset_glass_keys_follow_the_surface_tint_and_blur_mapping() {
        for preset in [DARK, LIGHT] {
            let values = preset.settings_values();
            let value = |key: &str| {
                values
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.clone())
            };
            assert_eq!(
                value("glassOpacityPercent"),
                Some(SettingValue::Percent(preset.tint_percent))
            );
            let blur_px = (f32::from(preset.blur_percent) / 100.0 * BLUR_MAX_PX).round() as u8;
            assert_eq!(preset.glass_blur_px, blur_px);
            assert_eq!(value("glassBlurPx"), Some(SettingValue::Percent(blur_px)));
        }
        assert_eq!((DARK.glass_opacity_percent, DARK.glass_blur_px), (12, 28));
        assert_eq!((LIGHT.glass_opacity_percent, LIGHT.glass_blur_px), (5, 13));
    }

    #[test]
    fn presets_write_each_surface_key_once_in_order() {
        let keys: Vec<_> = DARK.settings_values().iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "surfaceColor",
                "surfaceOpacity",
                "surfaceBlurPercent",
                "surfaceBorderColor",
                "surfaceBorderOpacity",
                "surfaceSaturationPercent",
                "surfaceRimFadePercent",
                "surfaceShadowPercent",
                "surfaceHighlightPercent",
                "glassOpacityPercent",
                "glassBlurPx",
            ]
        );
        assert_eq!(
            LIGHT.settings_values()[0].1,
            SettingValue::Text("#ffffff".into())
        );
    }
}

/// Canonical material role a widget declares. Consumers never pick RGBA or blur themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialRole {
    StandardContent,
    ContrastZone,
    GlassRegular,
    GlassClear,
    WindowChrome,
    MenuBar,
    Dock,
}

/// Taskbar/Dock material mode (`taskbarMaterialMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarMode {
    BlurOnly,
    Inherit,
    CustomTint,
}

/// Resolved appearance inputs, one per persisted setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    /// `glassifyContrastZones`.
    pub glassify_contrast_zones: bool,
    /// `reduceTransparency`.
    pub reduce_transparency: bool,
    /// `glassOpacityPercent` (4..=42).
    pub glass_opacity_percent: u8,
    /// `glassBlurPx` (0..=54).
    pub glass_blur_px: u8,
    /// `taskbarMaterialMode`.
    pub taskbar_mode: TaskbarMode,
    /// `taskbarOpacityPercent` (0..=75).
    pub taskbar_opacity_percent: u8,
    /// `taskbarCustomTint` RGB.
    pub taskbar_custom_tint: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            glassify_contrast_zones: false,
            reduce_transparency: false,
            glass_opacity_percent: 12,
            glass_blur_px: 28,
            taskbar_mode: TaskbarMode::BlurOnly,
            taskbar_opacity_percent: 8,
            taskbar_custom_tint: 0x5879ee,
        }
    }
}

/// Renderer capabilities that affect the resolved material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererCaps {
    /// False when the active path cannot sample the backdrop; blur is then dropped, alpha kept.
    pub backdrop_blur: bool,
}

/// Final material for one surface. `alpha_permille` keeps the 4.5% clear-glass token exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMaterial {
    pub tint_rgb: u32,
    pub alpha_permille: u16,
    /// Backdrop blur in device px at `output_scale_percent`.
    pub blur_px: u16,
}

impl ResolvedMaterial {
    #[must_use]
    pub const fn is_opaque(self) -> bool {
        self.alpha_permille >= 1000
    }
}

/// Dark-theme solid chrome and the opaque fallback color.
pub const STANDARD_RGB: u32 = 0x0017_2135;
const CONTRAST_RGB: u32 = 0x000f_1726;
const CONTRAST_GLASS_RGB: u32 = 0x0018_253e;
const CONTRAST_ALPHA_PERMILLE: u16 = 880;
const CONTRAST_GLASS_ALPHA_PERMILLE: u16 = 280;
const CONTRAST_GLASS_BLUR_PX: u16 = 26;
const CLEAR_ALPHA_PERMILLE: u16 = 45;
const CLEAR_BLUR_PX: u16 = 13;
const WINDOW_CHROME_RGB: u32 = 0x000f_1726;
const WINDOW_CHROME_ALPHA_PERMILLE: u16 = 600;
const WINDOW_CHROME_BLUR_PX: u16 = 17;
const NEUTRAL_RGB: u32 = 0x00ff_ffff;
/// Minimum Dock/Taskbar blur, and the glass blur offset it follows.
pub const TASKBAR_BLUR_MIN_PX: u16 = 16;
pub const TASKBAR_BLUR_OFFSET_PX: u16 = 6;

/// Blur radius in device px: logical px times `output_scale_percent / 100`, rounded.
#[must_use]
pub fn scale_px(logical: u16, output_scale_percent: u16) -> u16 {
    let scaled = u32::from(logical) * u32::from(output_scale_percent);
    u16::try_from((scaled + 50) / 100).unwrap_or(u16::MAX)
}

/// Regular glass material from the glass opacity and blur settings.
fn regular(appearance: Appearance) -> (u32, u16, u16) {
    (
        GLASS_TINT_RGB,
        u16::from(appearance.glass_opacity_percent) * 10,
        u16::from(appearance.glass_blur_px),
    )
}

/// Glass tint is the Dark preset tint; regular glass uses the user opacity and blur.
const GLASS_TINT_RGB: u32 = DARK.tint_rgb;

/// Taskbar blur: `max(16, glass_blur + 6)` logical px.
#[must_use]
pub fn taskbar_blur_px(glass_blur_px: u8) -> u16 {
    (u16::from(glass_blur_px) + TASKBAR_BLUR_OFFSET_PX).max(TASKBAR_BLUR_MIN_PX)
}

/// Pure role resolver. Precedence: reduce transparency, then Dock, then glassified ContrastZone,
/// then the role default. Blur is scaled by output scale and dropped without backdrop support.
#[must_use]
pub fn resolve(
    role: MaterialRole,
    appearance: Appearance,
    output_scale_percent: u16,
    caps: RendererCaps,
) -> ResolvedMaterial {
    let (tint_rgb, alpha_permille, logical_blur) = if appearance.reduce_transparency {
        let opaque = match role {
            MaterialRole::Dock if appearance.taskbar_mode == TaskbarMode::CustomTint => {
                appearance.taskbar_custom_tint
            }
            _ => STANDARD_RGB,
        };
        return ResolvedMaterial {
            tint_rgb: opaque,
            alpha_permille: 1000,
            blur_px: 0,
        };
    } else {
        match role {
            MaterialRole::StandardContent => (STANDARD_RGB, 1000, 0),
            MaterialRole::ContrastZone if appearance.glassify_contrast_zones => (
                CONTRAST_GLASS_RGB,
                CONTRAST_GLASS_ALPHA_PERMILLE,
                CONTRAST_GLASS_BLUR_PX,
            ),
            MaterialRole::ContrastZone => (CONTRAST_RGB, CONTRAST_ALPHA_PERMILLE, 0),
            MaterialRole::GlassRegular => {
                let (rgb, alpha, blur) = regular(appearance);
                (rgb, alpha, blur)
            }
            MaterialRole::GlassClear | MaterialRole::MenuBar => {
                (NEUTRAL_RGB, CLEAR_ALPHA_PERMILLE, CLEAR_BLUR_PX)
            }
            MaterialRole::WindowChrome => {
                (WINDOW_CHROME_RGB, WINDOW_CHROME_ALPHA_PERMILLE, WINDOW_CHROME_BLUR_PX)
            }
            MaterialRole::Dock => match appearance.taskbar_mode {
                TaskbarMode::Inherit => {
                    let (rgb, alpha, blur) = regular(appearance);
                    (rgb, alpha, blur)
                }
                TaskbarMode::BlurOnly => (
                    NEUTRAL_RGB,
                    u16::from(appearance.taskbar_opacity_percent) * 10,
                    taskbar_blur_px(appearance.glass_blur_px),
                ),
                TaskbarMode::CustomTint => (
                    appearance.taskbar_custom_tint,
                    u16::from(appearance.taskbar_opacity_percent) * 10,
                    taskbar_blur_px(appearance.glass_blur_px),
                ),
            },
        }
    };
    let blur_px = if caps.backdrop_blur {
        scale_px(logical_blur, output_scale_percent)
    } else {
        0
    };
    ResolvedMaterial {
        tint_rgb,
        alpha_permille,
        blur_px,
    }
}

/// Resolver input from the persisted glass settings. The glass keys are the only authority. Legacy
/// `surfaceOpacity` / `surfaceBlurPercent` seed the glass values only when the glass key is absent.
#[must_use]
pub fn appearance_from_settings(snapshot: &SettingsSnapshot) -> Appearance {
    let defaults = Appearance::default();
    let percent = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => Some(*value),
        _ => None,
    };
    let flag = |key: &str, default: bool| match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    };
    let mode = match snapshot.values.get("taskbarMaterialMode") {
        Some(SettingValue::Text(value)) if value == "inherit" => TaskbarMode::Inherit,
        Some(SettingValue::Text(value)) if value == "custom-tint" => TaskbarMode::CustomTint,
        _ => TaskbarMode::BlurOnly,
    };
    let legacy_blur_px = percent("surfaceBlurPercent")
        .map(|value| (u16::from(value.min(100)) * 40 / 100) as u8);
    let glass_blur_px = match (percent_u8_key(snapshot, "glassBlurPx"), legacy_blur_px) {
        (Some(px), _) => px,
        (None, Some(px)) => px,
        (None, None) => defaults.glass_blur_px,
    }
    .min(54);
    let glass_opacity_percent = percent("glassOpacityPercent")
        .or_else(|| percent("surfaceOpacity"))
        .unwrap_or(defaults.glass_opacity_percent)
        .min(100);
    let custom_tint = match snapshot.values.get("taskbarCustomTint") {
        Some(SettingValue::Text(value)) => parse_hex(value),
        _ => None,
    };
    Appearance {
        glassify_contrast_zones: flag("glassifyContrastZones", defaults.glassify_contrast_zones),
        reduce_transparency: flag("reduceTransparency", defaults.reduce_transparency),
        glass_opacity_percent,
        glass_blur_px,
        taskbar_mode: mode,
        taskbar_opacity_percent: percent("taskbarOpacityPercent")
            .unwrap_or(defaults.taskbar_opacity_percent)
            .min(75),
        taskbar_custom_tint: custom_tint.unwrap_or(defaults.taskbar_custom_tint),
    }
}

/// Glass-blur percent for backdrop consumers: `blur_px / BLUR_MAX_PX * 100`, capped at 100.
#[must_use]
pub fn blur_percent_from_px(blur_px: u16) -> u8 {
    let percent = (u32::from(blur_px) * 100 + 20) / 40;
    u8::try_from(percent.min(100)).unwrap_or(100)
}

fn percent_u8_key(snapshot: &SettingsSnapshot, key: &str) -> Option<u8> {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => Some(*value),
        _ => None,
    }
}

fn parse_hex(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod resolver_tests {
    use super::*;

    const ALL_ROLES: [MaterialRole; 7] = [
        MaterialRole::StandardContent,
        MaterialRole::ContrastZone,
        MaterialRole::GlassRegular,
        MaterialRole::GlassClear,
        MaterialRole::WindowChrome,
        MaterialRole::MenuBar,
        MaterialRole::Dock,
    ];
    const MODES: [TaskbarMode; 3] = [
        TaskbarMode::BlurOnly,
        TaskbarMode::Inherit,
        TaskbarMode::CustomTint,
    ];
    const CAPS: RendererCaps = RendererCaps { backdrop_blur: true };

    fn with(
        glassify: bool,
        reduce: bool,
        mode: TaskbarMode,
    ) -> Appearance {
        Appearance {
            glassify_contrast_zones: glassify,
            reduce_transparency: reduce,
            taskbar_mode: mode,
            ..Appearance::default()
        }
    }

    #[test]
    fn defaults_match_handoff() {
        let a = Appearance::default();
        assert!(!a.glassify_contrast_zones && !a.reduce_transparency);
        assert_eq!((a.glass_opacity_percent, a.glass_blur_px), (12, 28));
        assert_eq!(a.taskbar_mode, TaskbarMode::BlurOnly);
        assert_eq!((a.taskbar_opacity_percent, a.taskbar_custom_tint), (8, 0x5879ee));
    }

    #[test]
    fn standard_content_is_solid_and_never_glassified() {
        for glassify in [false, true] {
            let m = resolve(MaterialRole::StandardContent, with(glassify, false, TaskbarMode::BlurOnly), 100, CAPS);
            assert_eq!(m, ResolvedMaterial { tint_rgb: STANDARD_RGB, alpha_permille: 1000, blur_px: 0 });
        }
    }

    #[test]
    fn contrast_zone_is_semi_opaque_by_default_and_glassy_when_on() {
        let off = resolve(MaterialRole::ContrastZone, with(false, false, TaskbarMode::BlurOnly), 100, CAPS);
        assert_eq!((off.alpha_permille, off.blur_px), (880, 0));
        let on = resolve(MaterialRole::ContrastZone, with(true, false, TaskbarMode::BlurOnly), 100, CAPS);
        assert_eq!((on.tint_rgb, on.alpha_permille, on.blur_px), (CONTRAST_GLASS_RGB, 280, 26));
    }

    #[test]
    fn dock_is_never_implicitly_glassified() {
        // Glassify on must not change BlurOnly dock beyond its own mode.
        let off = resolve(MaterialRole::Dock, with(false, false, TaskbarMode::BlurOnly), 100, CAPS);
        let on = resolve(MaterialRole::Dock, with(true, false, TaskbarMode::BlurOnly), 100, CAPS);
        assert_eq!(off, on);
        assert_eq!((off.tint_rgb, off.alpha_permille, off.blur_px), (NEUTRAL_RGB, 80, 34));
    }

    #[test]
    fn taskbar_modes_are_independent_of_content_glass() {
        let base = Appearance { glass_opacity_percent: 30, glass_blur_px: 4, ..Appearance::default() };
        let regular_ = resolve(MaterialRole::GlassRegular, base, 100, CAPS);
        let inherit = resolve(MaterialRole::Dock, Appearance { taskbar_mode: TaskbarMode::Inherit, ..base }, 100, CAPS);
        assert_eq!(inherit, regular_);
        let blur = resolve(MaterialRole::Dock, base, 100, CAPS);
        assert_eq!((blur.tint_rgb, blur.alpha_permille, blur.blur_px), (NEUTRAL_RGB, 80, 16));
        let custom = resolve(
            MaterialRole::Dock,
            Appearance { taskbar_mode: TaskbarMode::CustomTint, taskbar_opacity_percent: 50, ..base },
            100,
            CAPS,
        );
        assert_eq!((custom.tint_rgb, custom.alpha_permille), (0x5879ee, 500));
    }

    #[test]
    fn reduce_transparency_wins_over_every_role_mode_and_glassify() {
        for role in ALL_ROLES {
            for mode in MODES {
                for glassify in [false, true] {
                    let m = resolve(role, with(glassify, true, mode), 150, CAPS);
                    assert!(m.is_opaque(), "{role:?} {mode:?} glassify={glassify}");
                    assert_eq!(m.blur_px, 0, "{role:?} {mode:?} glassify={glassify}");
                }
            }
        }
    }

    #[test]
    fn reduce_transparency_keeps_custom_tint_only_for_dock() {
        let a = with(false, true, TaskbarMode::CustomTint);
        assert_eq!(resolve(MaterialRole::Dock, a, 100, CAPS).tint_rgb, 0x5879ee);
        assert_eq!(resolve(MaterialRole::GlassRegular, a, 100, CAPS).tint_rgb, STANDARD_RGB);
    }

    #[test]
    fn blur_scales_with_output_scale_and_drops_without_caps() {
        let m = resolve(MaterialRole::GlassRegular, Appearance::default(), 150, CAPS);
        assert_eq!(m.blur_px, 42);
        let no = resolve(MaterialRole::GlassRegular, Appearance::default(), 150, RendererCaps { backdrop_blur: false });
        assert_eq!((no.blur_px, no.alpha_permille), (0, 120));
    }

    #[test]
    fn taskbar_blur_floor_and_offset() {
        assert_eq!(taskbar_blur_px(0), 16);
        assert_eq!(taskbar_blur_px(10), 16);
        assert_eq!(taskbar_blur_px(28), 34);
        assert_eq!(taskbar_blur_px(54), 60);
    }

    #[test]
    fn every_role_and_mode_resolves_within_bounds() {
        for role in ALL_ROLES {
            for mode in MODES {
                for glassify in [false, true] {
                    let m = resolve(role, with(glassify, false, mode), 100, CAPS);
                    assert!(m.alpha_permille <= 1000);
                    assert!(m.blur_px <= 60);
                }
            }
        }
    }
}
