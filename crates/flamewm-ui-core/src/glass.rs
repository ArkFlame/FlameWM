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
//! Dark: regular glass `rgba(234,244,255,.12)`, 28px blur, saturate 170%, rim `.17`.
//! Light: clear glass `rgba(255,255,255,.045)`, 13px blur, saturate 155%, rim `.23`.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::settings_groups::is_group_custom;
use flamewm_skin::palette::{
    BODY_MIN_CONTRAST, LIGHT_PANEL, LIGHT_SURFACE, Overlays, Palette, Rgb, contrast_ratio,
    effective_backdrop, glass_ink, scrim_alpha_floor_with,
};

/// Default `surfaceColor` RGB (neutral black, chroma zero), parsed from the single owner.
pub const DEFAULT_SURFACE_RGB: u32 = 0x0000_0000;

/// Tahoe corner radii (px at 1x). Consumers pick the one matching their surface.
/// The window frame radius has a single owner: `flamewm_skin::recipes::decoration::ROUNDED_RADIUS`.
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
    /// `surfaceOutlineColor` rim RGB.
    pub rim_rgb: u32,
    /// `surfaceOutlineOpacity` rim alpha percent.
    pub rim_percent: u8,
    /// `surfaceSaturationPercent`: backdrop saturation, 100 is identity.
    pub saturation_percent: u8,
    /// `glassOpacityPercent` (4..=100): `GlassRegular` tint alpha. Same value as `tint_percent`,
    /// because the surface tint and the glass regular tint come from one CSS token.
    pub glass_opacity_percent: u8,
    /// `glassBlurPx` (0..=54): `GlassRegular` blur in logical px. Derived from `blur_percent`
    /// as `blur_percent / 100 * BLUR_MAX_PX`, rounded: Dark 70% -> 28px, Light 33% -> 13px.
    pub glass_blur_px: u8,
}

/// Shared rim alpha (`surfaceOutlineOpacity` default, and `--rim` in every glass sheet as `#ffffff7f`).
/// The Settings help text and platform default read this same value.
pub const RIM_PERCENT: u8 = 50;

pub const DARK: GlassPreset = GlassPreset {
    tint_rgb: 0x00ea_f4ff,
    tint_percent: 12,
    blur_percent: 70,
    rim_rgb: 0x00ff_ffff,
    rim_percent: RIM_PERCENT,
    saturation_percent: 170,
    glass_opacity_percent: 12,
    glass_blur_px: 28,
};

pub const LIGHT: GlassPreset = GlassPreset {
    tint_rgb: 0x00ff_ffff,
    tint_percent: 5,
    blur_percent: 33,
    rim_rgb: 0x00ff_ffff,
    rim_percent: RIM_PERCENT,
    saturation_percent: 155,
    glass_opacity_percent: 5,
    glass_blur_px: 13,
};

impl GlassPreset {
    /// Settings keys and values this preset writes, in one fixed order.
    #[must_use]
    pub fn settings_values(self) -> [(&'static str, SettingValue); 9] {
        [
            // Liquid Glass is opt-in: the preset must lift the flat default, or it would stay opaque.
            ("reduceTransparency", SettingValue::Boolean(false)),
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
                "surfaceOutlineColor",
                SettingValue::Text(format!("#{:06x}", self.rim_rgb)),
            ),
            (
                "surfaceOutlineOpacity",
                SettingValue::Percent(self.rim_percent),
            ),
            (
                "surfaceSaturationPercent",
                SettingValue::Percent(self.saturation_percent),
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

    const _: () = assert!(LIGHT.tint_percent < DARK.tint_percent);

    #[test]
    fn dark_is_the_tahoe_default_and_light_is_clear() {
        assert_eq!(DARK.tint_rgb, 0x00ea_f4ff);
        assert_eq!(DARK.blur_percent, 70);
        assert_eq!(LIGHT.rim_percent, DARK.rim_percent);
        assert_eq!(DARK.rim_percent, RIM_PERCENT);
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
                "reduceTransparency",
                "surfaceColor",
                "surfaceOpacity",
                "surfaceBlurPercent",
                "surfaceOutlineColor",
                "surfaceOutlineOpacity",
                "surfaceSaturationPercent",
                "glassOpacityPercent",
                "glassBlurPx",
            ]
        );
        assert_eq!(
            LIGHT.settings_values()[1].1,
            SettingValue::Text("#ffffff".into())
        );
        assert_eq!(
            DARK.settings_values()[0].1,
            SettingValue::Boolean(false),
            "a preset turns the flat default off"
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
    /// `reduceTransparency`.
    pub reduce_transparency: bool,
    /// `glassOpacityPercent` (4..=100).
    pub glass_opacity_percent: u8,
    /// Regular glass tint RGB: the `surfaceColor` when the Surfaces custom switch is on, else the
    /// shared default (neutral black). Chroma zero by default, so the translucent base is not blue.
    pub tint_rgb: u32,
    /// `glassBlurPx` (0..=54).
    pub glass_blur_px: u8,
    /// `taskbarMaterialMode`.
    pub taskbar_mode: TaskbarMode,
    /// `taskbarOpacityPercent` (0..=75).
    pub taskbar_opacity_percent: u8,
    /// `taskbarCustomTint` RGB.
    pub taskbar_custom_tint: u32,
    /// `appearance` is `light` (case-insensitive). `system` and `dark` resolve to the Dark palette.
    pub light: bool,
    /// Derived `wallpaperMeanRgb`: the backdrop glass shows. `None` until the desktop publishes it,
    /// which keeps the material-only ink and no scrim.
    pub backdrop_rgb: Option<u32>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            reduce_transparency: false,
            glass_opacity_percent: flamewm_api::settings_groups::DEFAULT_GLASS_OPACITY,
            tint_rgb: DEFAULT_SURFACE_RGB,
            glass_blur_px: 28,
            taskbar_mode: TaskbarMode::Inherit,
            taskbar_opacity_percent: 8,
            taskbar_custom_tint: 0x0058_79ee,
            light: false,
            backdrop_rgb: None,
        }
    }
}

/// Permille (0..=1000) to a tint alpha byte (0..=255), rounded to nearest.
fn permille_to_alpha(permille: u16) -> u8 {
    ((u32::from(permille.min(1000)) * 255 + 500) / 1000) as u8
}

/// Tint alpha byte to permille, rounded up so a floor is never undershot.
fn alpha_to_permille(alpha: u8) -> u16 {
    (u32::from(alpha) * 1000).div_ceil(255) as u16
}

/// Scrim that keeps the body and muted ink at 4.5:1 over the published backdrop: the scrim colour
/// and the least permille that reaches it. `None` without a backdrop or when no scrim can reach it.
fn scrim_for(appearance: Appearance) -> Option<(u32, u16)> {
    let backdrop = Rgb(appearance.backdrop_rgb?);
    let palette = ink_for(appearance)?;
    let scrim = if palette == Palette::RWR_0_0_9 {
        Rgb(0)
    } else {
        Rgb(0x00ff_ffff)
    };
    let text = scrim_alpha_floor_with(palette.text, backdrop, scrim, BODY_MIN_CONTRAST)?;
    let muted = scrim_alpha_floor_with(palette.muted, backdrop, scrim, BODY_MIN_CONTRAST)?;
    Some((scrim.0, alpha_to_permille(text.max(muted))))
}

/// Ink palette for the glass over the published backdrop; `None` without one.
fn ink_for(appearance: Appearance) -> Option<Palette> {
    let backdrop = Rgb(appearance.backdrop_rgb?);
    let tint = Rgb(glass_tint_rgb(appearance));
    let alpha = permille_to_alpha(u16::from(appearance.glass_opacity_percent) * 10);
    Some(glass_ink(tint, alpha, backdrop, BODY_MIN_CONTRAST).palette)
}

/// Window backdrop `(opacity_percent, blur_percent)` for an app whose surfaces follow
/// `surfaceOpacity` / `surfaceBlurPercent`. Flat (`reduceTransparency`, the default) is opaque with no
/// blur, so the wallpaper capture is never requested for it.
#[must_use]
pub fn window_backdrop_material(
    snapshot: &SettingsSnapshot,
    opacity_percent: u8,
    blur_percent: u8,
) -> (u8, u8) {
    if appearance_from_settings(snapshot).reduce_transparency {
        (100, 0)
    } else {
        (opacity_percent, blur_percent)
    }
}

/// Derived `wallpaperMeanRgb` as a packed rgb value.
fn wallpaper_mean_rgb(snapshot: &SettingsSnapshot) -> Option<u32> {
    match snapshot.values.get("wallpaperMeanRgb") {
        Some(SettingValue::Integer(rgb)) if (0..=0x00ff_ffff).contains(rgb) => Some(*rgb as u32),
        _ => None,
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
pub const STANDARD_RGB: u32 = 0x0020_2020;
const CONTRAST_GLASS_RGB: u32 = 0x0018_253e;
const CONTRAST_GLASS_ALPHA_PERMILLE: u16 = 280;
const CONTRAST_GLASS_BLUR_PX: u16 = 26;
const CLEAR_ALPHA_PERMILLE: u16 = 45;
const CLEAR_BLUR_PX: u16 = 13;
const WINDOW_CHROME_RGB: u32 = 0x001c_1c1c;
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

/// Opaque standard chrome for the resolved appearance (`Palette::LIGHT.panel` in light mode).
const fn standard_rgb(appearance: Appearance) -> u32 {
    if appearance.light {
        LIGHT_PANEL.0
    } else {
        STANDARD_RGB
    }
}

/// Regular glass material from the glass opacity and blur settings.
fn regular(appearance: Appearance) -> (u32, u16, u16) {
    let tint = glass_tint_rgb(appearance);
    let alpha = u16::from(appearance.glass_opacity_percent) * 10;
    let blur = u16::from(appearance.glass_blur_px);
    // Legibility is judged on the surface actually drawn: the user tint at the user alpha over the
    // backdrop. The ink palette is chosen on that same surface, so only a failing surface takes the
    // scrim, and the scrim floor is solved over the raw backdrop it replaces the tint on.
    let Some(palette) = ink_for(appearance) else {
        return (tint, alpha, blur);
    };
    let backdrop = Rgb(appearance.backdrop_rgb.unwrap_or_default());
    let drawn = effective_backdrop(Rgb(tint), permille_to_alpha(alpha), backdrop);
    let legible = contrast_ratio(palette.text, drawn) >= BODY_MIN_CONTRAST
        && contrast_ratio(palette.muted, drawn) >= BODY_MIN_CONTRAST;
    if legible {
        return (tint, alpha, blur);
    }
    // The scrim never drops below the user opacity, so the setting still applies above the floor.
    match scrim_for(appearance) {
        Some((rgb, floor)) => (rgb, floor.max(alpha), blur),
        None => (tint, alpha, blur),
    }
}

/// Regular glass tint: the resolved `surfaceColor` (neutral black by default), or
/// `Palette::LIGHT.surface` in light mode.
const fn glass_tint_rgb(appearance: Appearance) -> u32 {
    if appearance.light {
        LIGHT_SURFACE.0
    } else {
        appearance.tint_rgb
    }
}

/// Taskbar blur: `max(16, glass_blur + 6)` logical px.
#[must_use]
pub fn taskbar_blur_px(glass_blur_px: u8) -> u16 {
    (u16::from(glass_blur_px) + TASKBAR_BLUR_OFFSET_PX).max(TASKBAR_BLUR_MIN_PX)
}

/// Pure role resolver. Precedence: reduce transparency, then Dock, then the role default.
/// `ContrastZone` is always glassified; it is not a setting. Blur is scaled by output scale and dropped without backdrop support.
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
            _ => standard_rgb(appearance),
        };
        return ResolvedMaterial {
            tint_rgb: opaque,
            alpha_permille: 1000,
            blur_px: 0,
        };
    } else {
        match role {
            MaterialRole::StandardContent => (standard_rgb(appearance), 1000, 0),
            MaterialRole::ContrastZone => (
                CONTRAST_GLASS_RGB,
                CONTRAST_GLASS_ALPHA_PERMILLE,
                CONTRAST_GLASS_BLUR_PX,
            ),
            MaterialRole::GlassRegular => {
                let (rgb, alpha, blur) = regular(appearance);
                (rgb, alpha, blur)
            }
            MaterialRole::GlassClear | MaterialRole::MenuBar => {
                (NEUTRAL_RGB, CLEAR_ALPHA_PERMILLE, CLEAR_BLUR_PX)
            }
            MaterialRole::WindowChrome => (
                if appearance.light {
                    LIGHT_PANEL.0
                } else {
                    WINDOW_CHROME_RGB
                },
                WINDOW_CHROME_ALPHA_PERMILLE,
                WINDOW_CHROME_BLUR_PX,
            ),
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
        Some(SettingValue::Text(value)) if value == "blur-only" => TaskbarMode::BlurOnly,
        _ => defaults.taskbar_mode,
    };
    let legacy_blur_px =
        percent("surfaceBlurPercent").map(|value| (u16::from(value.min(100)) * 40 / 100) as u8);
    let glass_blur_px = match (percent_u8_key(snapshot, "glassBlurPx"), legacy_blur_px) {
        (Some(px), _) => px,
        (None, Some(px)) => px,
        (None, None) => defaults.glass_blur_px,
    }
    .min(54);
    // The Surfaces slider writes surfaceOpacity only, so while the glass group is not custom the
    // surface key is the live value and the glass mirror is stale. A custom glass group owns its key.
    let glass_opacity_percent = if flag("glassCustom", false) {
        percent("glassOpacityPercent").or_else(|| percent("surfaceOpacity"))
    } else {
        percent("surfaceOpacity").or_else(|| percent("glassOpacityPercent"))
    }
    .unwrap_or(defaults.glass_opacity_percent)
    .min(100);
    let custom_tint = match snapshot.values.get("taskbarCustomTint") {
        Some(SettingValue::Text(value)) => flamewm_api::color::parse_hex_u32(value),
        _ => None,
    };
    // Same rule as `surface_fill`: the stored colour applies only with the Surfaces custom switch on.
    let tint_rgb = match snapshot.values.get("surfaceColor") {
        Some(SettingValue::Text(value)) if is_group_custom(snapshot, "surface") => {
            flamewm_api::color::parse_hex_u32(value).unwrap_or(defaults.tint_rgb)
        }
        _ => defaults.tint_rgb,
    };
    Appearance {
        reduce_transparency: flag("reduceTransparency", defaults.reduce_transparency),
        glass_opacity_percent,
        tint_rgb,
        glass_blur_px,
        taskbar_mode: mode,
        taskbar_opacity_percent: percent("taskbarOpacityPercent")
            .unwrap_or(defaults.taskbar_opacity_percent)
            .min(75),
        taskbar_custom_tint: custom_tint.unwrap_or(defaults.taskbar_custom_tint),
        light: matches!(
            snapshot.values.get("appearance"),
            Some(SettingValue::Text(value)) if value.eq_ignore_ascii_case("light")
        ),
        backdrop_rgb: wallpaper_mean_rgb(snapshot),
    }
}

/// Overlay tokens (`--rim`, `--hover-fill`, `--pressed-fill`) for the snapshot's appearance.
/// Shares the selector with `palette_from_settings` so both follow one `appearance` value.
#[must_use]
pub fn overlays_from_settings(snapshot: &SettingsSnapshot) -> Overlays {
    if appearance_from_settings(snapshot).light {
        Overlays::LIGHT
    } else {
        Overlays::DARK
    }
}

/// Neutral text and surface tokens for the snapshot's appearance. Dark returns the shipped
/// palette; `appearance=light` returns `Palette::LIGHT`. Shared by every consumer that projects
/// `--text`, `--muted`, `--panel`, `--surface` or `--sidebar` at runtime.
#[must_use]
pub fn palette_from_settings(snapshot: &SettingsSnapshot) -> Palette {
    let appearance = appearance_from_settings(snapshot);
    if !appearance.reduce_transparency
        && let Some(palette) = ink_for(appearance)
    {
        return palette;
    }
    if appearance.light {
        Palette::LIGHT
    } else {
        Palette::RWR_0_0_9
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

/// Translucent glass material: the Liquid Glass opt-in (`reduceTransparency` off). Test-only.
#[cfg(test)]
fn glass_on() -> Appearance {
    Appearance {
        reduce_transparency: false,
        ..Appearance::default()
    }
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
    const CAPS: RendererCaps = RendererCaps {
        backdrop_blur: true,
    };

    fn with(reduce: bool, mode: TaskbarMode) -> Appearance {
        Appearance {
            reduce_transparency: reduce,
            taskbar_mode: mode,
            ..glass_on()
        }
    }

    #[test]
    fn defaults_are_translucent_black_at_75_percent() {
        let a = Appearance::default();
        assert!(!a.reduce_transparency, "translucent is the default");
        assert_eq!(a.glass_opacity_percent, 75);
        assert_eq!(a.tint_rgb, 0x0000_0000, "black base, chroma zero");
        assert_eq!(a.glass_blur_px, 28, "blur stays at the existing default");
        assert_eq!(
            Some(DEFAULT_SURFACE_RGB),
            flamewm_api::color::parse_hex_u32(flamewm_api::settings_groups::DEFAULT_SURFACE_COLOR),
            "the glass default must match the settings owner"
        );
        assert_eq!(
            a.glass_opacity_percent,
            flamewm_api::settings_groups::DEFAULT_SURFACE_OPACITY
        );
    }

    #[test]
    fn reduce_transparency_true_stays_opaque_with_default_tint() {
        let flat = Appearance {
            reduce_transparency: true,
            ..Appearance::default()
        };
        let m = resolve(MaterialRole::GlassRegular, flat, 100, CAPS);
        assert_eq!((m.alpha_permille, m.blur_px), (1000, 0));
    }

    #[test]
    fn default_glass_uses_black_tint_and_light_ink_on_dark_backdrop() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(appearance_from_settings(&snapshot).tint_rgb, 0);
        snapshot.values.insert(
            "wallpaperMeanRgb".into(),
            SettingValue::Integer(0x0010_1010),
        );
        let ink = palette_from_settings(&snapshot);
        assert_eq!(
            ink.text,
            Palette::RWR_0_0_9.text,
            "light text on black glass"
        );
    }

    #[test]
    fn surface_color_applies_only_with_surface_custom_on() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceColor".into(), SettingValue::Text("#eaf4ff".into()));
        assert_eq!(
            appearance_from_settings(&snapshot).tint_rgb,
            0,
            "custom off: default"
        );
        snapshot
            .values
            .insert("surfaceCustom".into(), SettingValue::Boolean(true));
        assert_eq!(
            appearance_from_settings(&snapshot).tint_rgb,
            0x00ea_f4ff,
            "custom on: stored colour"
        );
    }

    #[test]
    fn taskbar_defaults_are_unchanged() {
        let a = Appearance::default();
        assert_eq!(a.taskbar_mode, TaskbarMode::Inherit);
        assert_eq!(
            (a.taskbar_opacity_percent, a.taskbar_custom_tint),
            (8, 0x0058_79ee)
        );
    }

    #[test]
    fn missing_or_invalid_taskbar_mode_inherits_surface_glass() {
        let empty = SettingsSnapshot::new(0);
        assert_eq!(
            appearance_from_settings(&empty).taskbar_mode,
            TaskbarMode::Inherit
        );
    }

    #[test]
    fn standard_content_is_solid() {
        let m = resolve(
            MaterialRole::StandardContent,
            with(false, TaskbarMode::BlurOnly),
            100,
            CAPS,
        );
        assert_eq!(
            m,
            ResolvedMaterial {
                tint_rgb: STANDARD_RGB,
                alpha_permille: 1000,
                blur_px: 0
            }
        );
    }

    #[test]
    fn contrast_zone_is_always_glassified() {
        let m = resolve(
            MaterialRole::ContrastZone,
            with(false, TaskbarMode::BlurOnly),
            100,
            CAPS,
        );
        assert_eq!(
            (m.tint_rgb, m.alpha_permille, m.blur_px),
            (CONTRAST_GLASS_RGB, 280, 26)
        );
    }

    #[test]
    fn dock_blur_only_uses_its_own_material() {
        let m = resolve(
            MaterialRole::Dock,
            with(false, TaskbarMode::BlurOnly),
            100,
            CAPS,
        );
        assert_eq!(
            (m.tint_rgb, m.alpha_permille, m.blur_px),
            (NEUTRAL_RGB, 80, 34)
        );
    }

    #[test]
    fn taskbar_modes_are_independent_of_content_glass() {
        let base = Appearance {
            glass_opacity_percent: 30,
            glass_blur_px: 4,
            ..glass_on()
        };
        let regular_ = resolve(MaterialRole::GlassRegular, base, 100, CAPS);
        let inherit = resolve(
            MaterialRole::Dock,
            Appearance {
                taskbar_mode: TaskbarMode::Inherit,
                ..base
            },
            100,
            CAPS,
        );
        assert_eq!(inherit, regular_);
        let blur = resolve(
            MaterialRole::Dock,
            Appearance {
                taskbar_mode: TaskbarMode::BlurOnly,
                ..base
            },
            100,
            CAPS,
        );
        assert_eq!(
            (blur.tint_rgb, blur.alpha_permille, blur.blur_px),
            (NEUTRAL_RGB, 80, 16)
        );
        let custom = resolve(
            MaterialRole::Dock,
            Appearance {
                taskbar_mode: TaskbarMode::CustomTint,
                taskbar_opacity_percent: 50,
                ..base
            },
            100,
            CAPS,
        );
        assert_eq!((custom.tint_rgb, custom.alpha_permille), (0x0058_79ee, 500));
    }

    #[test]
    fn reduce_transparency_wins_over_every_role_and_mode() {
        for role in ALL_ROLES {
            for mode in MODES {
                let m = resolve(role, with(true, mode), 150, CAPS);
                assert!(m.is_opaque(), "{role:?} {mode:?}");
                assert_eq!(m.blur_px, 0, "{role:?} {mode:?}");
            }
        }
    }

    #[test]
    fn reduce_transparency_keeps_custom_tint_only_for_dock() {
        let a = with(true, TaskbarMode::CustomTint);
        assert_eq!(
            resolve(MaterialRole::Dock, a, 100, CAPS).tint_rgb,
            0x0058_79ee
        );
        assert_eq!(
            resolve(MaterialRole::GlassRegular, a, 100, CAPS).tint_rgb,
            STANDARD_RGB
        );
    }

    #[test]
    fn blur_scales_with_output_scale_and_drops_without_caps() {
        let m = resolve(MaterialRole::GlassRegular, glass_on(), 150, CAPS);
        assert_eq!(m.blur_px, 42);
        let no = resolve(
            MaterialRole::GlassRegular,
            glass_on(),
            150,
            RendererCaps {
                backdrop_blur: false,
            },
        );
        assert_eq!((no.blur_px, no.alpha_permille), (0, 750));
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
                let m = resolve(role, with(false, mode), 100, CAPS);
                assert!(m.alpha_permille <= 1000);
                assert!(m.blur_px <= 60);
            }
        }
    }
}

#[cfg(test)]
mod appearance_golden_tests {
    use super::*;

    fn snapshot_with_appearance(value: Option<&str>) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        if let Some(value) = value {
            snapshot
                .values
                .insert("appearance".into(), SettingValue::Text(value.into()));
        }
        snapshot
    }

    #[test]
    fn neutral_palette_tokens_follow_appearance() {
        assert_eq!(
            palette_from_settings(&snapshot_with_appearance(None)),
            Palette::RWR_0_0_9
        );
        assert_eq!(
            palette_from_settings(&snapshot_with_appearance(Some("light"))),
            Palette::LIGHT
        );
    }

    fn regular_at(appearance: Appearance) -> ResolvedMaterial {
        resolve(
            MaterialRole::GlassRegular,
            appearance,
            100,
            RendererCaps {
                backdrop_blur: true,
            },
        )
    }

    /// Dark default output is bit-identical to the pre-light-mode resolver values.
    #[test]
    fn default_and_dark_materials_are_bit_identical() {
        for snapshot in [
            snapshot_with_appearance(None),
            snapshot_with_appearance(Some("dark")),
            snapshot_with_appearance(Some("system")),
        ] {
            // Translucent default golden: black base at 75%, blur 28px, no tint chroma.
            let appearance = Appearance {
                reduce_transparency: false,
                ..appearance_from_settings(&snapshot)
            };
            assert!(!appearance.light);
            assert_eq!(
                regular_at(appearance),
                ResolvedMaterial {
                    tint_rgb: 0x0000_0000,
                    alpha_permille: 750,
                    blur_px: 28,
                }
            );
            let standard = resolve(
                MaterialRole::StandardContent,
                appearance,
                100,
                RendererCaps {
                    backdrop_blur: true,
                },
            );
            assert_eq!(standard.tint_rgb, 0x0020_2020);
            let chrome = resolve(
                MaterialRole::WindowChrome,
                appearance,
                100,
                RendererCaps {
                    backdrop_blur: true,
                },
            );
            assert_eq!(chrome.tint_rgb, 0x001c_1c1c);
        }
        assert_eq!(
            appearance_from_settings(&snapshot_with_appearance(None)),
            Appearance::default()
        );
    }

    #[test]
    fn light_appearance_uses_palette_light_tokens() {
        let appearance = Appearance {
            reduce_transparency: false,
            ..appearance_from_settings(&snapshot_with_appearance(Some("LIGHT")))
        };
        assert!(appearance.light);
        assert_eq!(regular_at(appearance).tint_rgb, LIGHT_SURFACE.0);
        let standard = resolve(
            MaterialRole::StandardContent,
            appearance,
            100,
            RendererCaps {
                backdrop_blur: true,
            },
        );
        assert_eq!(standard.tint_rgb, LIGHT_PANEL.0);
        let opaque = resolve(
            MaterialRole::GlassRegular,
            Appearance {
                reduce_transparency: true,
                ..appearance
            },
            100,
            RendererCaps {
                backdrop_blur: true,
            },
        );
        assert_eq!(opaque.tint_rgb, LIGHT_PANEL.0);
        assert!(opaque.is_opaque());
    }
}

#[cfg(test)]
mod backdrop_ink_tests {
    use super::*;
    use flamewm_skin::palette::{AlphaColor, composite, contrast_ratio};

    const BODY: f64 = 4.5;
    /// Bright pink wallpaper under Settings Dark sidebar labels (measured 2.0 to 2.2:1 before).
    const BRIGHT_PINK: u32 = 0x00ff_b6c1;
    /// Measured Task Manager Light backdrop sample (upper end).
    const MID_VIOLET: u32 = 0x0071_6c87;
    const CAPS: RendererCaps = RendererCaps {
        backdrop_blur: true,
    };

    fn with_backdrop(light: bool, rgb: u32, opacity: u8) -> Appearance {
        Appearance {
            light,
            backdrop_rgb: Some(rgb),
            glass_opacity_percent: opacity,
            ..glass_on()
        }
    }

    #[test]
    fn no_published_backdrop_keeps_the_material_only_ink() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("appearance".into(), SettingValue::Text("light".into()));
        assert_eq!(palette_from_settings(&snapshot), Palette::LIGHT);
        assert_eq!(appearance_from_settings(&snapshot).backdrop_rgb, None);
    }

    #[test]
    fn published_backdrop_is_read_from_the_derived_key() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "wallpaperMeanRgb".into(),
            SettingValue::Integer(0x00ff_b6c1),
        );
        assert_eq!(
            appearance_from_settings(&snapshot).backdrop_rgb,
            Some(BRIGHT_PINK)
        );
        snapshot
            .values
            .insert("wallpaperMeanRgb".into(), SettingValue::Integer(-1));
        assert_eq!(appearance_from_settings(&snapshot).backdrop_rgb, None);
    }

    #[test]
    fn bright_wallpaper_flips_dark_appearance_to_dark_ink() {
        assert_eq!(
            ink_for(with_backdrop(false, BRIGHT_PINK, 12)),
            Some(Palette::LIGHT)
        );
    }

    #[test]
    fn dark_wallpaper_keeps_shipped_ink_and_needs_no_scrim() {
        let appearance = with_backdrop(false, 0, 12);
        assert_eq!(ink_for(appearance), Some(Palette::RWR_0_0_9));
        assert_eq!(scrim_for(appearance), Some((0, 0)));
        let material = resolve(MaterialRole::GlassRegular, appearance, 100, CAPS);
        assert_eq!(material.alpha_permille, 120);
        assert_eq!(material.tint_rgb, glass_tint_rgb(appearance));
    }

    #[test]
    fn mid_wallpaper_gets_a_scrim_that_reaches_body_contrast() {
        let appearance = with_backdrop(true, MID_VIOLET, 5);
        let material = resolve(MaterialRole::GlassRegular, appearance, 100, CAPS);
        assert!(material.alpha_permille > 50, "{material:?}");
        let palette = ink_for(appearance).expect("backdrop present");
        let surface = composite(
            AlphaColor {
                color: Rgb(material.tint_rgb),
                alpha: permille_to_alpha(material.alpha_permille),
            },
            Rgb(MID_VIOLET),
        );
        assert!(contrast_ratio(palette.text, surface) >= BODY);
        assert!(contrast_ratio(palette.muted, surface) >= BODY);
    }

    #[test]
    fn user_opacity_above_the_floor_is_kept() {
        let appearance = with_backdrop(true, MID_VIOLET, 42);
        let material = resolve(MaterialRole::GlassRegular, appearance, 100, CAPS);
        assert!(material.alpha_permille >= 420, "{material:?}");
    }

    #[test]
    fn drawn_regular_glass_meets_body_contrast_over_appearance_and_backdrop_grid() {
        const LEVELS: [u32; 6] = [0x00, 0x33, 0x66, 0x99, 0xcc, 0xff];
        for light in [false, true] {
            for opacity in [4_u8, 8, 12, 22, 33, 42] {
                for r in LEVELS {
                    for g in LEVELS {
                        for b in LEVELS {
                            let rgb = (r << 16) | (g << 8) | b;
                            let appearance = with_backdrop(light, rgb, opacity);
                            let material =
                                resolve(MaterialRole::GlassRegular, appearance, 100, CAPS);
                            let palette = ink_for(appearance).expect("backdrop present");
                            let surface = composite(
                                AlphaColor {
                                    color: Rgb(material.tint_rgb),
                                    alpha: permille_to_alpha(material.alpha_permille),
                                },
                                Rgb(rgb),
                            );
                            assert!(
                                contrast_ratio(palette.text, surface) >= BODY,
                                "text light={light} opacity={opacity} rgb={rgb:06x} {material:?}"
                            );
                            assert!(
                                contrast_ratio(palette.muted, surface) >= BODY,
                                "muted light={light} opacity={opacity} rgb={rgb:06x} {material:?}"
                            );
                            assert!(
                                material.alpha_permille >= u16::from(opacity) * 10,
                                "user opacity is the floor: {material:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn alpha_conversions_round_so_a_floor_is_not_undershot() {
        assert_eq!(permille_to_alpha(1000), 255);
        assert_eq!(alpha_to_permille(255), 1000);
        assert!(alpha_to_permille(permille_to_alpha(500)) >= 500);
    }
}

#[cfg(test)]
mod translucent_default_gate_tests {
    use super::*;

    #[test]
    fn unset_settings_resolve_every_material_translucent_black_with_blur() {
        let snapshot = SettingsSnapshot::new(1);
        let appearance = appearance_from_settings(&snapshot);
        assert!(!appearance.reduce_transparency);
        for role in [MaterialRole::GlassRegular, MaterialRole::Dock] {
            let material = resolve(
                role,
                appearance,
                100,
                RendererCaps {
                    backdrop_blur: true,
                },
            );
            assert_eq!(material.tint_rgb, 0, "{role:?} black base");
            assert_eq!(material.alpha_permille, 750, "{role:?} 75%");
            assert_eq!(material.blur_px, 28, "{role:?} keeps the existing blur");
        }
    }

    #[test]
    fn reduce_transparency_true_keeps_every_material_opaque_without_blur() {
        let snapshot = {
            let mut s = SettingsSnapshot::new(1);
            s.values
                .insert("reduceTransparency".into(), SettingValue::Boolean(true));
            s
        };
        let appearance = appearance_from_settings(&snapshot);
        for role in [
            MaterialRole::GlassRegular,
            MaterialRole::Dock,
            MaterialRole::WindowChrome,
        ] {
            let material = resolve(
                role,
                appearance,
                100,
                RendererCaps {
                    backdrop_blur: true,
                },
            );
            assert!(material.is_opaque(), "{role:?}");
            assert_eq!(material.blur_px, 0, "{role:?}");
        }
    }

    #[test]
    fn window_backdrop_is_translucent_by_default_and_opaque_only_when_flat() {
        let translucent = SettingsSnapshot::new(1);
        assert_eq!(window_backdrop_material(&translucent, 75, 70), (75, 70));
        let flat = {
            let mut s = SettingsSnapshot::new(1);
            s.values
                .insert("reduceTransparency".into(), SettingValue::Boolean(true));
            s
        };
        assert_eq!(window_backdrop_material(&flat, 75, 70), (100, 0));
    }

    #[test]
    fn explicit_glass_opt_in_keeps_translucent_material() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("reduceTransparency".into(), SettingValue::Boolean(false));
        let material = resolve(
            MaterialRole::GlassRegular,
            appearance_from_settings(&snapshot),
            100,
            RendererCaps {
                backdrop_blur: true,
            },
        );
        assert!(!material.is_opaque());
        assert_eq!(material.blur_px, 28);
    }
}

#[cfg(test)]
mod surface_opacity_mapping_tests {
    use super::*;
    use crate::surface::surface_fill;

    fn snapshot(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in entries {
            snapshot.values.insert((*key).into(), value.clone());
        }
        snapshot
    }

    #[test]
    fn surfaces_opacity_wins_over_a_stale_glass_mirror_when_glass_is_not_custom() {
        // The Surfaces slider writes only surfaceOpacity; glassOpacityPercent keeps its old 75.
        let snapshot = snapshot(&[
            ("surfaceCustom", SettingValue::Boolean(true)),
            ("surfaceOpacity", SettingValue::Percent(67)),
            ("glassOpacityPercent", SettingValue::Percent(75)),
        ]);
        assert_eq!(
            appearance_from_settings(&snapshot).glass_opacity_percent,
            67
        );
        assert_eq!(surface_fill(&snapshot).alpha, 171);
    }

    #[test]
    fn custom_glass_group_keeps_its_own_opacity() {
        let snapshot = snapshot(&[
            ("glassCustom", SettingValue::Boolean(true)),
            ("surfaceOpacity", SettingValue::Percent(67)),
            ("glassOpacityPercent", SettingValue::Percent(75)),
        ]);
        assert_eq!(
            appearance_from_settings(&snapshot).glass_opacity_percent,
            75
        );
    }
}
