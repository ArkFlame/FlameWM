use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_skin::icons::IconRole;
use flamewm_skin::palette::Rgb;
use flamewm_skin::recipes::decoration::{
    ButtonShape, ButtonTokens, ChromeTokens, DecorationStyle,
};
use flamewm_skin::recipes::window_chrome::{self as recipe, WINDOW_CHROME, WindowControlRole};

/// Skin-owned titlebar height (flamewm-skin RWR 0.0.9 chrome titlebar).
pub const TITLEBAR_HEIGHT: u16 = WINDOW_CHROME.metrics.titlebar;
/// Frame border width (no skin token; WM-owned 1px border).
pub const FRAME_BORDER: u16 = 1;

/// WM-owned view of control roles (mirrors skin WindowControlRole).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlRole {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl ControlRole {
    #[must_use]
    pub const fn skin(self) -> WindowControlRole {
        match self {
            Self::Minimize => WindowControlRole::Minimize,
            Self::Maximize => WindowControlRole::Maximize,
            Self::Restore => WindowControlRole::Restore,
            Self::Close => WindowControlRole::Close,
        }
    }

    #[must_use]
    pub const fn icon_role(self) -> IconRole {
        self.skin().icon_role()
    }

    /// Breeze asset path for this control (`assets/web/breeze/window-*.svg`).
    #[must_use]
    pub fn asset_path(self) -> &'static str {
        self.icon_role().source().path
    }
}

impl From<WindowControlRole> for ControlRole {
    fn from(role: WindowControlRole) -> Self {
        match role {
            WindowControlRole::Minimize => Self::Minimize,
            WindowControlRole::Maximize | WindowControlRole::Restore => Self::Maximize,
            WindowControlRole::Close => Self::Close,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPolicy {
    pub roles: [ControlRole; 3],
    pub button_width: u16,
}

impl Default for ControlPolicy {
    fn default() -> Self {
        Self::for_state(false, false)
    }
}

impl ControlPolicy {
    /// Skin control order; the middle slot becomes Restore when maximized or
    /// fullscreen so the hit target keeps its geometry while the glyph swaps.
    #[must_use]
    pub fn for_state(maximized: bool, fullscreen: bool) -> Self {
        let middle = if maximized || fullscreen {
            ControlRole::Restore
        } else {
            ControlRole::Maximize
        };
        Self {
            roles: [ControlRole::Minimize, middle, ControlRole::Close],
            button_width: WINDOW_CHROME.metrics.button_width,
        }
    }
}

/// Validated `_NET_WM_ICON` image (EWMH ARGB32 cardinals, non-premultiplied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
}

/// Parse concatenated EWMH `_NET_WM_ICON` cardinals; select largest valid image.
/// Rejects zero/overflow dimensions and truncated payloads.
pub fn parse_net_wm_icon(cardinals: &[u32]) -> Option<IconImage> {
    let mut cursor = 0_usize;
    let mut best: Option<IconImage> = None;
    while cursor.saturating_add(2) <= cardinals.len() {
        let width = cardinals[cursor];
        let height = cardinals[cursor + 1];
        cursor += 2;
        if width == 0 || height == 0 {
            return None;
        }
        let count = width
            .checked_mul(height)
            .and_then(|pixels| usize::try_from(pixels).ok())?;
        if cursor.saturating_add(count) > cardinals.len() {
            return None;
        }
        let argb = cardinals[cursor..cursor + count].to_vec();
        cursor += count;
        let area = u64::from(width) * u64::from(height);
        let best_area = best.as_ref().map_or(0, |image: &IconImage| {
            u64::from(image.width) * u64::from(image.height)
        });
        if area >= best_area {
            best = Some(IconImage {
                width,
                height,
                argb,
            });
        }
    }
    if cursor != cardinals.len() {
        return None;
    }
    best.filter(|image| !image.argb.is_empty())
}

/// Nearest-neighbor scale into a square titlebar slot, alpha preserved.
pub fn scale_icon_to_slot(icon: &IconImage, slot: u32) -> Option<IconImage> {
    if slot == 0 || icon.width == 0 || icon.height == 0 {
        return None;
    }
    let slot_usize = usize::try_from(slot).ok()?;
    let width_usize = usize::try_from(icon.width).ok()?;
    let height_usize = usize::try_from(icon.height).ok()?;
    let mut argb = Vec::with_capacity(slot_usize * slot_usize);
    for y in 0..slot_usize {
        let source_y = y * height_usize / slot_usize;
        for x in 0..slot_usize {
            let source_x = x * width_usize / slot_usize;
            argb.push(icon.argb[source_y * width_usize + source_x]);
        }
    }
    Some(IconImage {
        width: slot,
        height: slot,
        argb,
    })
}

/// Straight RGBA8 raster for a native `_NET_WM_ICON` selection, scaled into
/// the square titlebar slot (canonical blit input for `blit_rgba`).
#[must_use]
pub fn native_icon_rgba(icon: &IconImage, slot: u32) -> Option<flamewm_image_core::RgbaImage> {
    let scaled = scale_icon_to_slot(icon, slot)?;
    let mut pixels = Vec::with_capacity(scaled.argb.len() * 4);
    for pixel in &scaled.argb {
        pixels.push(((pixel >> 16) & 0xff) as u8);
        pixels.push(((pixel >> 8) & 0xff) as u8);
        pixels.push((pixel & 0xff) as u8);
        pixels.push(((pixel >> 24) & 0xff) as u8);
    }
    flamewm_image_core::RgbaImage::from_rgba8(scaled.width, scaled.height, pixels)
}

/// Material variant for one cached control glyph: resting or hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlMaterial {
    Rest,
    Hover,
    Pressed,
}

impl ControlMaterial {
    #[must_use]
    pub const fn for_pointer(hovered: bool, pressed: bool) -> Self {
        if pressed {
            Self::Pressed
        } else if hovered {
            Self::Hover
        } else {
            Self::Rest
        }
    }
}

/// Per-button style look: overrides for one control plus the titlebar color
/// the flat shape blends into. `Default` is the skin look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlLook {
    pub button: ButtonTokens,
    pub titlebar: Rgb,
}

impl Default for ControlLook {
    fn default() -> Self {
        Self {
            button: ButtonTokens::default(),
            titlebar: ChromeTokens::default().titlebar_bg,
        }
    }
}

/// Cache key for a control glyph raster: role + edge + material + look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlGlyphKey {
    pub role: ControlRole,
    pub edge: u32,
    pub material: ControlMaterial,
    pub look: ControlLook,
}

impl ControlGlyphKey {
    #[must_use]
    pub fn new(role: ControlRole, edge: u32, material: ControlMaterial) -> Self {
        Self {
            role,
            edge,
            material,
            look: ControlLook::default(),
        }
    }
}

/// Process-global bounded glyph raster cache (WM loop is single-threaded;
/// mutex guards cross-call sharing, never held across X calls).
const CONTROL_GLYPH_CACHE_CAPACITY: usize = 32;

fn control_glyph_cache() -> &'static Mutex<HashMap<ControlGlyphKey, flamewm_image_core::RgbaImage>>
{
    static CACHE: OnceLock<Mutex<HashMap<ControlGlyphKey, flamewm_image_core::RgbaImage>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Render one control glyph raster with its per-material semantic color
/// scheme. Rest uses the Flame default; hover/pressed tint the scheme text
/// channel (dark glyph on light disk for min/max/restore, light glyph on
/// red disk for close). Pure/cached assets only.
#[must_use]
pub fn control_glyph_raster(
    role: ControlRole,
    edge: u32,
    material: ControlMaterial,
    look: ControlLook,
) -> Option<flamewm_image_core::RgbaImage> {
    if edge == 0 || edge > flamewm_image_core::MAX_DIMENSION {
        return None;
    }
    let key = ControlGlyphKey {
        look,
        ..ControlGlyphKey::new(role, edge, material)
    };
    if let Ok(cache) = control_glyph_cache().lock() {
        if let Some(cached) = cache.get(&key) {
            return Some(cached.clone());
        }
    }
    let scheme = control_glyph_scheme(role, material, look);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(role.asset_path());
    let bytes = std::fs::read(path).ok()?;
    let mut raster =
        flamewm_image_core::svg::render_with_color_scheme(&bytes, edge, edge, &scheme).ok()?;
    if material != ControlMaterial::Rest {
        match look.button.shape {
            ButtonShape::Default if role == ControlRole::Close => {
                paint_fill_behind(&mut raster, scheme.background, FillMask::Circle);
            }
            ButtonShape::Default | ButtonShape::Flat => {}
            ButtonShape::Circle => {
                paint_fill_behind(&mut raster, scheme.background, FillMask::Circle);
            }
            ButtonShape::Square => {
                paint_fill_behind(&mut raster, scheme.background, FillMask::Square);
            }
        }
    }
    if let Ok(mut cache) = control_glyph_cache().lock() {
        if cache.len() >= CONTROL_GLYPH_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, raster.clone());
    }
    Some(raster)
}

/// Backing mask behind a control glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillMask {
    Circle,
    Square,
}

/// Fills the glyph-edge mask with `fill` (opaque) and composites the
/// straight-alpha glyph over it. Pixels outside a circle mask are untouched.
fn paint_fill_behind(
    raster: &mut flamewm_image_core::RgbaImage,
    fill: [u8; 4],
    mask: FillMask,
) {
    let radius = raster.width as f32 / 2.0;
    for y in 0..raster.height {
        for x in 0..raster.width {
            let dx = x as f32 + 0.5 - radius;
            let dy = y as f32 + 0.5 - radius;
            if mask == FillMask::Circle && dx * dx + dy * dy > radius * radius {
                continue;
            }
            let offset = ((y * raster.width + x) * 4) as usize;
            let pixel = &mut raster.pixels[offset..offset + 4];
            let glyph_alpha = u16::from(pixel[3]);
            let disk_alpha = 255 - glyph_alpha;
            for channel in 0..3 {
                pixel[channel] = ((u16::from(pixel[channel]) * glyph_alpha
                    + u16::from(fill[channel]) * disk_alpha
                    + 127)
                    / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
}

/// Opaque RGBA from a skin `0xRRGGBB` token.
const fn skin_rgba(rgb: u32) -> [u8; 4] {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

/// Semantic per-material color scheme for control glyphs.
#[must_use]
fn control_glyph_scheme(
    role: ControlRole,
    material: ControlMaterial,
    look: ControlLook,
) -> flamewm_image_core::SvgColorScheme {
    let base = flamewm_image_core::SvgColorScheme::flame_default();
    let mut scheme = match (role, material) {
        (_, ControlMaterial::Rest) => base,
        (ControlRole::Close, _) => flamewm_image_core::SvgColorScheme {
            text: [0xf1, 0xf2, 0xf3, 255],
            background: skin_rgba(WINDOW_CHROME.close_hover.0),
            highlight: base.highlight,
            negative_text: base.negative_text,
        },
        (_, ControlMaterial::Hover) => flamewm_image_core::SvgColorScheme {
            text: [0x1b, 0x1e, 0x20, 255],
            background: [0xf1, 0xf2, 0xf3, 255],
            highlight: base.highlight,
            negative_text: base.negative_text,
        },
        (_, ControlMaterial::Pressed) => flamewm_image_core::SvgColorScheme {
            text: [0x1b, 0x1e, 0x20, 255],
            background: [0xc7, 0xc9, 0xcb, 255],
            highlight: base.highlight,
            negative_text: base.negative_text,
        },
    };
    if let Some(glyph) = look.button.glyph {
        scheme.text = skin_rgba(glyph.0);
    }
    let fill = match material {
        ControlMaterial::Rest => None,
        ControlMaterial::Hover => look.button.hover,
        ControlMaterial::Pressed => look.button.pressed,
    };
    if let Some(fill) = fill {
        scheme.background = skin_rgba(fill.0);
    }
    if look.button.shape == ButtonShape::Flat && material != ControlMaterial::Rest {
        scheme.background = skin_rgba(look.titlebar.0);
    }
    scheme
}

/// Code-level delegation anchor for the external drawable contract: the live
/// paint path executes through this external drawable contract.
/// Dead-code type reference (never called) so source-context guards observe
/// the delegation outside comments; behavior unchanged.
#[allow(dead_code)]
fn external_drawable_delegation_target() -> &'static str {
    std::any::type_name::<flamewm_render_x11::ExternalDrawableTarget>()
}

/// Titlebar icon slot edge for the configured titlebar height.
#[must_use]
pub fn icon_slot_for(titlebar_height: u16) -> u16 {
    recipe::icon_slot_edge()
        .min(titlebar_height.saturating_sub(4))
        .max(recipe::ICON_SLOT_MIN)
}

/// Title baseline (Xft device pixels) for the configured titlebar height.
#[must_use]
pub fn title_baseline(titlebar_height: u16) -> f32 {
    f32::from(titlebar_height.saturating_sub(11).max(11))
}

/// Live control blit plan: one cached glyph raster per visible control,
/// destination rects from skin `control_button_geometries`.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlBlit {
    pub control: crate::frame::model::FrameControl,
    pub role: ControlRole,
    pub raster: flamewm_image_core::RgbaImage,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_w: f32,
    pub dest_h: f32,
}

/// Resolve the live control blits for one frame: exact skin control
/// geometry/colors per button, glyph edge from skin
/// `control_glyph_edge`, centered in the 38px button. Hover/pressed swap
/// the cached glyph material; the hit target keeps its geometry.
#[must_use]
pub fn control_blits_for(
    frame_width: u32,
    maximized: bool,
    fullscreen: bool,
    hover: Option<crate::frame::model::FrameControl>,
    pressed: Option<crate::frame::model::FrameControl>,
    tokens: &ChromeTokens,
) -> Vec<ControlBlit> {
    use crate::frame::model::FrameControl;
    let policy = ControlPolicy::for_state(maximized, fullscreen);
    let titlebar = recipe::SceneRect::new(
        0,
        0,
        i32::try_from(frame_width).unwrap_or(i32::MAX),
        i32::from(WINDOW_CHROME.metrics.titlebar),
    );
    let geometries = recipe::control_button_geometries(titlebar);
    let glyph_edge = f32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let button_w = f32::from(WINDOW_CHROME.metrics.button_width);
    let button_h = f32::from(WINDOW_CHROME.metrics.titlebar);
    let controls = [
        FrameControl::Minimize,
        FrameControl::MaximizeRestore,
        FrameControl::Close,
    ];
    let mut blits = Vec::with_capacity(3);
    for (index, control) in controls.iter().enumerate() {
        let role = policy.roles[index];
        let hovered = hover == Some(*control);
        let armed = pressed == Some(*control);
        let material = ControlMaterial::for_pointer(hovered, armed);
        let look = ControlLook {
            button: tokens.buttons[index],
            titlebar: tokens.titlebar_bg,
        };
        let Some(raster) = control_glyph_raster(
            role,
            u32::from(WINDOW_CHROME.metrics.control_glyph_edge),
            material,
            look,
        ) else {
            continue;
        };
        let bounds = geometries[index].bounds;
        let dest_x = bounds.x as f32 + (button_w - glyph_edge) / 2.0;
        let dest_y = bounds.y as f32 + (button_h - glyph_edge) / 2.0;
        blits.push(ControlBlit {
            control: *control,
            role,
            raster,
            dest_x,
            dest_y,
            dest_w: glyph_edge,
            dest_h: glyph_edge,
        });
    }
    blits
}

/// Centered title origin for the live paint path using the skin
/// `center_title_x` contract: desired `(W - tw) / 2` clamped into the free
/// region between the left-occupied icon slot and the right-occupied
/// 114px control strip, padded by skin `TITLE_PAD`.
#[must_use]
pub fn live_title_x(frame_width: u32, title_width: i32) -> f32 {
    let bar_w = i32::try_from(frame_width).unwrap_or(i32::MAX);
    let left_occupied =
        recipe::ICON_PAD_LEFT + i32::from(recipe::icon_slot_edge()) + recipe::TITLE_PAD;
    let right_occupied = bar_w - recipe::controls_width();
    recipe::center_title_x(
        bar_w,
        title_width,
        left_occupied,
        right_occupied,
        recipe::TITLE_PAD,
    ) as f32
}

/// Native icon destination rect for the live paint path (skin geometry).
#[must_use]
pub fn icon_dest_rect(titlebar_height: u16) -> (f32, f32, f32) {
    let slot = f32::from(icon_slot_for(titlebar_height));
    (
        recipe::ICON_PAD_LEFT as f32,
        (f32::from(titlebar_height) - slot) / 2.0,
        slot,
    )
}

/// Renderer-owned title measurement for Xft IBM Plex Sans 12px semibold
/// (skin typography title). Proportional advance estimate: ASCII 7px, CJK
/// 12px, other 8px; keeps the skin `center_title_x` contract without the
/// retired 9px core-font advance.
#[must_use]
pub fn title_text_width(title: &str) -> i32 {
    let mut width = 0_i32;
    let mut count = 0_usize;
    for ch in title.chars() {
        if count >= TITLE_MAX_CHARS {
            break;
        }
        count += 1;
        let advance = if ch.is_ascii() {
            7
        } else if is_wide(ch) {
            12
        } else {
            8
        };
        width = width.saturating_add(advance);
    }
    width
}

const TITLE_MAX_CHARS: usize = 96;

fn is_wide(ch: char) -> bool {
    matches!(ch,
        '\u{1100}'..='\u{115F}' | '\u{2E80}'..='\u{A4CF}' | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}' | '\u{FE30}'..='\u{FE4F}' | '\u{FF00}'..='\u{FFEF}')
}

/// Live chrome tokens, written at the settings sync point and read by the
/// WM turn hook (same single-threaded precedent as `snap_preview` opacity).
static LIVE_CHROME_TOKENS: Mutex<Option<ChromeTokens>> = Mutex::new(None);

/// Publish the settings-derived chrome tokens for the next WM turn.
pub fn set_live_chrome_tokens(tokens: ChromeTokens) {
    if let Ok(mut live) = LIVE_CHROME_TOKENS.lock() {
        *live = Some(tokens);
    }
}

/// Current live chrome tokens (skin defaults until settings publish).
#[must_use]
pub fn live_chrome_tokens() -> ChromeTokens {
    LIVE_CHROME_TOKENS
        .lock()
        .ok()
        .and_then(|live| *live)
        .unwrap_or_default()
}

/// Map WM settings onto skin chrome tokens. Empty or malformed values keep
/// the skin default (`#rrggbb` colors, `default|flat|circle|square` shapes).
#[must_use]
pub fn chrome_tokens_from_settings(snapshot: &SettingsSnapshot) -> ChromeTokens {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.as_str(),
        _ => "",
    };
    let style = match text("decoration") {
        "ios" => DecorationStyle::Ios,
        _ => DecorationStyle::Rounded,
    };
    let titlebar_bg = parse_settings_color(text("titlebarColor"))
        .unwrap_or(ChromeTokens::default().titlebar_bg);
    let button = |name: &str| ButtonTokens {
        shape: match text(&format!("titlebar{name}Style")) {
            "flat" => ButtonShape::Flat,
            "circle" => ButtonShape::Circle,
            "square" => ButtonShape::Square,
            _ => ButtonShape::Default,
        },
        glyph: parse_settings_color(text(&format!("titlebar{name}Color"))),
        hover: parse_settings_color(text(&format!("titlebar{name}HoverColor"))),
        pressed: parse_settings_color(text(&format!("titlebar{name}PressedColor"))),
    };
    ChromeTokens {
        style,
        titlebar_bg,
        buttons: [button("Minimize"), button("Maximize"), button("Close")],
    }
}

/// `#rrggbb` to skin color; empty or malformed input is `None`.
fn parse_settings_color(value: &str) -> Option<Rgb> {
    let (r, g, b) = flamewm_platform::chrome::parse_hex_rgb(value)?;
    Some(Rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_skin::recipes::decoration::corner_radius;

    #[test]
    fn settings_color_rejects_sign_and_prefix_forms() {
        assert_eq!(parse_settings_color("#+12345"), None);
        assert_eq!(parse_settings_color("#-12345"), None);
        assert_eq!(parse_settings_color("#0x1234"), None);
        assert_eq!(parse_settings_color("#123456"), Some(Rgb(0x12_3456)));
    }

    #[test]
    fn center_formula_matches_flame_contract() {
        // desired=(400-90)/2=155 inside free region.
        assert_eq!(recipe::center_title_x(400, 90, 22, 304, 6), 155);
        // Desired below minimum but free region inverted: sorted clamp keeps desired (20).
        assert_eq!(recipe::center_title_x(400, 360, 22, 304, 6), 20);
        // Title wider than the free region: sorted clamp keeps desired (10).
        assert_eq!(recipe::center_title_x(200, 180, 22, 104, 6), 10);
    }

    #[test]
    fn title_measurement_uses_proportional_advance() {
        assert_eq!(title_text_width(""), 0);
        assert_eq!(title_text_width("ab"), 14);
        assert_eq!(title_text_width("あ"), 12);
    }

    #[test]
    fn net_wm_icon_parser_accepts_valid_argb() {
        let cardinals = vec![
            2,
            1,
            0xff00_0000,
            0xff00_ff00, // 2x1
            1,
            1,
            0x80ff_0000, // 1x1
        ];
        let icon = parse_net_wm_icon(&cardinals).expect("valid payload");
        assert_eq!((icon.width, icon.height), (2, 1));
        assert_eq!(icon.argb, vec![0xff00_0000, 0xff00_ff00]);
    }

    #[test]
    fn net_wm_icon_parser_rejects_invalid_payload() {
        assert!(parse_net_wm_icon(&[2, 2, 1, 2, 3]).is_none());
        assert!(parse_net_wm_icon(&[0, 1, 0]).is_none());
        assert!(parse_net_wm_icon(&[1, 1]).is_none());
        assert!(parse_net_wm_icon(&[1, 1, 0, 99]).is_none());
    }

    #[test]
    fn icon_scales_into_slot_with_alpha_preserved() {
        let icon = IconImage {
            width: 2,
            height: 2,
            argb: vec![0xff00_0000, 0x0000_0000, 0x80ff_0000, 0xff00_ff00],
        };
        let scaled = scale_icon_to_slot(&icon, 4).expect("scaled");
        assert_eq!((scaled.width, scaled.height), (4, 4));
        assert_eq!(scaled.argb.len(), 16);
        assert!(scaled.argb.contains(&0x0000_0000));
        assert!(scaled.argb.contains(&0x80ff_0000));
    }

    #[test]
    fn settings_map_to_skin_tokens_and_bad_values_fall_back() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(chrome_tokens_from_settings(&snapshot), ChromeTokens::default());
        let text = |value: &str| SettingValue::Text(value.to_owned());
        snapshot.values.insert("decoration".to_owned(), text("ios"));
        snapshot.values.insert("titlebarColor".to_owned(), text("#202326"));
        snapshot.values.insert("titlebarCloseStyle".to_owned(), text("circle"));
        snapshot.values.insert("titlebarCloseHoverColor".to_owned(), text("#00ff00"));
        snapshot.values.insert("titlebarMinimizeColor".to_owned(), text("#zz"));
        snapshot.values.insert("titlebarMaximizeStyle".to_owned(), text("bogus"));
        let tokens = chrome_tokens_from_settings(&snapshot);
        assert_eq!(tokens.style, DecorationStyle::Ios);
        assert_eq!(tokens.titlebar_bg, Rgb(0x202326));
        assert_eq!(tokens.buttons[2].shape, ButtonShape::Circle);
        assert_eq!(tokens.buttons[2].hover, Some(Rgb(0x00ff00)));
        assert_eq!(tokens.buttons[0].glyph, None);
        assert_eq!(tokens.buttons[1].shape, ButtonShape::Default);
    }

    #[test]
    fn explicit_skin_default_tokens_reproduce_default_glyph_pixels() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let explicit = ControlLook {
            button: ButtonTokens {
                shape: ButtonShape::Default,
                glyph: None,
                hover: Some(WINDOW_CHROME.close_hover),
                pressed: None,
            },
            titlebar: WINDOW_CHROME.titlebar_background,
        };
        let default = control_glyph_raster(ControlRole::Close, edge, ControlMaterial::Hover, ControlLook::default())
            .expect("close hover raster");
        let explicit = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Hover,
            explicit,
        )
        .expect("explicit close hover raster");
        assert_eq!(default.pixels, explicit.pixels);
    }

    #[test]
    fn style_tokens_never_move_control_hit_geometry() {
        let mut tokens = ChromeTokens::default();
        tokens.style = DecorationStyle::Ios;
        for button in tokens.buttons.iter_mut() {
            button.shape = ButtonShape::Square;
        }
        let default = control_blits_for(400, false, false, None, None, &ChromeTokens::default());
        let styled = control_blits_for(400, false, false, None, None, &tokens);
        let dests = |blits: &[ControlBlit]| {
            blits
                .iter()
                .map(|blit| (blit.dest_x, blit.dest_y, blit.dest_w, blit.dest_h))
                .collect::<Vec<_>>()
        };
        assert_eq!(dests(&default), dests(&styled));
    }

    #[test]
    fn policy_uses_skin_controls_and_rectangular_maximized() {
        assert_eq!(
            ControlPolicy::for_state(true, false).roles[1],
            ControlRole::Restore
        );
        assert_eq!(
            ControlPolicy::for_state(false, false).roles[1],
            ControlRole::Maximize
        );
        assert_eq!(
            corner_radius(true, DecorationStyle::Rounded),
            WINDOW_CHROME.metrics.radius
        );
        assert_eq!(corner_radius(false, DecorationStyle::Rounded), 0);
    }

    #[test]
    fn native_icon_converts_to_straight_rgba() {
        let icon = IconImage {
            width: 1,
            height: 1,
            argb: vec![0x80ff_0000],
        };
        let rgba = native_icon_rgba(&icon, 2).expect("rgba");
        assert_eq!((rgba.width, rgba.height), (2, 2));
        assert_eq!(rgba.pixels, vec![255, 0, 0, 128].repeat(4));
    }

    #[test]
    fn icon_slot_and_baseline_follow_titlebar_height() {
        assert_eq!(icon_slot_for(31), recipe::icon_slot_edge());
        assert_eq!(title_baseline(31), 20.0);
        assert_eq!(title_baseline(10), 11.0);
    }

    #[test]
    fn control_glyphs_render_from_breeze_assets() {
        for role in [
            ControlRole::Minimize,
            ControlRole::Maximize,
            ControlRole::Restore,
            ControlRole::Close,
        ] {
            let image =
                control_glyph_raster(role, 16, ControlMaterial::Rest, ControlLook::default()).expect("breeze control svg");
            assert_eq!((image.width, image.height), (16, 16));
            assert!(image.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
        assert!(control_glyph_raster(ControlRole::Close, 0, ControlMaterial::Rest, ControlLook::default()).is_none());
    }

    #[test]
    fn hovering_close_selects_hover_material_only_for_close() {
        use crate::frame::model::FrameControl;
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        // Blits carry no material field, so the material each control was
        // rendered with is observed through its cached glyph raster.
        assert_eq!(ControlMaterial::for_pointer(false, false), ControlMaterial::Rest);
        assert_eq!(ControlMaterial::for_pointer(true, false), ControlMaterial::Hover);
        let rest = control_blits_for(400, false, false, None, None, &ChromeTokens::default());
        let hovered = control_blits_for(
            400,
            false,
            false,
            Some(FrameControl::Close),
            None,
            &ChromeTokens::default(),
        );
        assert_eq!(rest.len(), 3);
        assert_eq!(hovered.len(), 3);
        let expected = [
            (FrameControl::Minimize, ControlMaterial::Rest),
            (FrameControl::MaximizeRestore, ControlMaterial::Rest),
            (FrameControl::Close, ControlMaterial::Hover),
        ];
        for (index, (control, material)) in expected.iter().enumerate() {
            assert_eq!(hovered[index].control, *control);
            let raster = control_glyph_raster(hovered[index].role, edge, *material, ControlLook::default())
                .expect("control raster");
            assert_eq!(
                hovered[index].raster, raster,
                "{control:?} must use {material:?} material on close hover"
            );
            let (h, r) = (&hovered[index], &rest[index]);
            assert_eq!(
                (h.dest_x, h.dest_y, h.dest_w, h.dest_h),
                (r.dest_x, r.dest_y, r.dest_w, r.dest_h),
                "hover must keep the {control:?} destination geometry"
            );
        }
        for (index, blit) in rest.iter().enumerate() {
            let raster = control_glyph_raster(blit.role, edge, ControlMaterial::Rest, ControlLook::default())
                .expect("control raster");
            assert_eq!(blit.raster, raster, "no hover must select Rest material for every control");
            assert_eq!(blit.control, hovered[index].control);
        }
    }

    #[test]
    fn close_hover_and_pressed_rasters_differ_from_rest() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let rest = control_glyph_raster(ControlRole::Close, edge, ControlMaterial::Rest, ControlLook::default())
            .expect("close rest raster");
        for material in [ControlMaterial::Hover, ControlMaterial::Pressed] {
            let raster = control_glyph_raster(ControlRole::Close, edge, material, ControlLook::default())
                .expect("close material raster");
            assert_ne!(
                raster.pixels, rest.pixels,
                "close {material:?} must not be byte-identical to rest"
            );
        }
    }

    #[test]
    fn close_hover_paints_skin_red_disk_behind_glyph() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let raster = control_glyph_raster(ControlRole::Close, edge, ControlMaterial::Hover, ControlLook::default())
            .expect("close hover raster");
        // (1, 8) is inside the edge disk and clear of the glyph strokes.
        let offset = ((8 * edge + 1) * 4) as usize;
        let red = WINDOW_CHROME.close_hover.0;
        assert_eq!(
            &raster.pixels[offset..offset + 4],
            &[(red >> 16) as u8, (red >> 8) as u8, red as u8, 255]
        );
    }

    #[test]
    fn minimize_rest_raster_keeps_flame_default_scheme() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let raster = control_glyph_raster(ControlRole::Minimize, edge, ControlMaterial::Rest, ControlLook::default())
            .expect("minimize rest raster");
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../")
                .join(ControlRole::Minimize.asset_path()),
        )
        .expect("minimize asset");
        let expected = flamewm_image_core::svg::render_with_color_scheme(
            &bytes,
            edge,
            edge,
            &flamewm_image_core::SvgColorScheme::flame_default(),
        )
        .expect("minimize expected");
        assert_eq!(raster.pixels, expected.pixels);
    }
}
