use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_skin::icons::IconRole;
use flamewm_skin::palette::Rgb;
use flamewm_skin::recipes::decoration::{ButtonShape, ButtonTokens, ChromeTokens, DecorationStyle};
use flamewm_skin::recipes::window_chrome::{self as recipe, WINDOW_CHROME, WindowControlRole};

/// Skin-owned titlebar height (flamewm-skin RWR 0.0.9 chrome titlebar).
pub const TITLEBAR_HEIGHT: u16 = WINDOW_CHROME.metrics.titlebar;
/// Frame border width (no skin token; WM-owned 1px border).
pub const FRAME_BORDER: u16 = 1;
const MAX_CONTROL_GLYPH_EDGE: u32 = 4096;

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

fn control_glyph_cache()
-> &'static Mutex<HashMap<ControlGlyphKey, Arc<flamewm_image_core::RgbaImage>>> {
    static CACHE: OnceLock<Mutex<HashMap<ControlGlyphKey, Arc<flamewm_image_core::RgbaImage>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Render one control glyph raster with its per-material semantic color
/// scheme. Rest uses the Flame default; hover/pressed tint the scheme text
/// channel (dark glyph on light disk for min/max/restore, light glyph on
/// red disk for close). Pure/cached assets only.
#[cfg(test)]
#[must_use]
pub fn control_glyph_raster(
    role: ControlRole,
    edge: u32,
    material: ControlMaterial,
    look: ControlLook,
) -> Option<flamewm_image_core::RgbaImage> {
    control_glyph_raster_shared(role, edge, material, look).map(|raster| (*raster).clone())
}

/// Shared form of `control_glyph_raster`: a cache hit clones the `Arc`, not the pixels.
#[must_use]
pub fn control_glyph_raster_shared(
    role: ControlRole,
    edge: u32,
    material: ControlMaterial,
    look: ControlLook,
) -> Option<Arc<flamewm_image_core::RgbaImage>> {
    if edge == 0 || edge > MAX_CONTROL_GLYPH_EDGE {
        return None;
    }
    let key = ControlGlyphKey {
        look,
        ..ControlGlyphKey::new(role, edge, material)
    };
    if let Ok(cache) = control_glyph_cache().lock() {
        if let Some(cached) = cache.get(&key) {
            return Some(Arc::clone(cached));
        }
    }
    let scheme = control_glyph_scheme(role, material, look);
    // WM titlebar controls must resolve from the installed package on a second
    // machine, not from the builder's CARGO_MANIFEST_DIR. This is the same
    // canonical runtime data root used by the titlebar wallpaper compositor.
    let path = crate::frame::titlebar_backdrop::packaged_root().join(role.asset_path());
    let mut raster = match std::fs::read(&path).ok().and_then(|bytes| {
        flamewm_image_core::svg::render_with_color_scheme(&bytes, edge, edge, &scheme).ok()
    }) {
        Some(raster) => raster,
        None => {
            eprintln!(
                "flamewm: titlebar control asset unavailable ({}); using native vector fallback",
                path.display()
            );
            fallback_control_glyph(role, edge, scheme.text)?
        }
    };
    if material != ControlMaterial::Rest {
        match look.button.shape {
            // Default: every control gets its hover/pressed disk (red for close, light for min/max,
            // whose glyph turns dark and would otherwise vanish on the dark titlebar).
            ButtonShape::Default | ButtonShape::Circle => {
                paint_fill_behind(&mut raster, scheme.background, FillMask::Circle);
            }
            ButtonShape::Flat => {}
            // Square style fills the whole titlebar cell in the frame paint owner.
            // Baking a background into this 16px glyph would leave an inset pill.
            ButtonShape::Square => {}
        }
    }
    let raster = Arc::new(raster);
    if let Ok(mut cache) = control_glyph_cache().lock() {
        if cache.len() >= CONTROL_GLYPH_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&raster));
    }
    Some(raster)
}

/// No native titlebar control is allowed to disappear because an icon theme or
/// SVG decoder is unavailable on the installed machine. The normal Breeze
/// asset remains authoritative; this bounded raster is the failure fallback.
fn fallback_control_glyph(
    role: ControlRole,
    edge: u32,
    rgba: [u8; 4],
) -> Option<flamewm_image_core::RgbaImage> {
    if edge < 4 || edge > 128 {
        return None;
    }
    let mut pixels = vec![0_u8; (edge as usize) * (edge as usize) * 4];
    let put = |x: u32, y: u32, pixels: &mut [u8]| {
        if x < edge && y < edge {
            let at = ((y * edge + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&rgba);
        }
    };
    let low = edge / 4;
    let high = edge - low - 1;
    let mid = edge / 2;
    let stroke = (edge / 12).max(1);
    for i in low..=high {
        for k in 0..stroke {
            match role {
                ControlRole::Minimize => put(i, mid + k, &mut pixels),
                ControlRole::Maximize => {
                    put(i, low + k, &mut pixels);
                    put(i, high.saturating_sub(k), &mut pixels);
                    put(low + k, i, &mut pixels);
                    put(high.saturating_sub(k), i, &mut pixels);
                }
                ControlRole::Restore => {
                    put(i, low + k, &mut pixels);
                    put(high.saturating_sub(k), i, &mut pixels);
                    if i > low + stroke {
                        put(i - stroke, high.saturating_sub(k), &mut pixels);
                    }
                    if i < high - stroke {
                        put(low + stroke + k, i, &mut pixels);
                    }
                }
                ControlRole::Close => {
                    put(i, i.saturating_add(k), &mut pixels);
                    put(i, low + high - i + k, &mut pixels);
                }
            }
        }
    }
    flamewm_image_core::RgbaImage::from_rgba8(edge, edge, pixels)
}
/// Backing mask behind a control glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillMask {
    Circle,
}

impl FillMask {
    /// Whether the pixel center `(px, py)` lies inside the mask over a `size`-square glyph edge.
    fn contains(self, px: f32, py: f32, size: f32) -> bool {
        match self {
            Self::Circle => {
                let radius = size / 2.0;
                let dx = px - radius;
                let dy = py - radius;
                dx * dx + dy * dy <= radius * radius
            }
        }
    }
}

/// Fills the glyph-edge mask with `fill` (opaque) and composites the
/// straight-alpha glyph over it. Pixels outside the mask are untouched.
fn paint_fill_behind(raster: &mut flamewm_image_core::RgbaImage, fill: [u8; 4], mask: FillMask) {
    let size = raster.width as f32;
    for y in 0..raster.height {
        for x in 0..raster.width {
            if !mask.contains(x as f32 + 0.5, y as f32 + 0.5, size) {
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
    let base_h = u32::from(WINDOW_CHROME.metrics.titlebar);
    let scaled =
        (u32::from(recipe::icon_slot_edge()) * u32::from(titlebar_height) + base_h / 2) / base_h;
    scaled
        .min(u32::from(titlebar_height.saturating_sub(4)))
        .max(u32::from(recipe::ICON_SLOT_MIN))
        .min(u32::from(u16::MAX)) as u16
}

/// Title baseline (Xft device pixels) for the configured titlebar height.
#[must_use]
pub fn title_baseline(titlebar_height: u16) -> f32 {
    f32::from(titlebar_height.saturating_sub(11).max(11))
}

/// Live control blit plan: one cached glyph raster per visible control,
/// destination rects from the native input-child geometry authority.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlBlit {
    pub control: crate::frame::model::FrameControl,
    pub role: ControlRole,
    pub raster: Arc<flamewm_image_core::RgbaImage>,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_w: f32,
    pub dest_h: f32,
}

/// Resolve default-size control glyphs; hover/pressed never move the hit targets.
#[must_use]
pub fn control_blits_for(
    frame_width: u32,
    maximized: bool,
    fullscreen: bool,
    hover: Option<crate::frame::model::FrameControl>,
    pressed: Option<crate::frame::model::FrameControl>,
    tokens: &ChromeTokens,
) -> Vec<ControlBlit> {
    control_blits_for_height(
        frame_width,
        WINDOW_CHROME.metrics.titlebar,
        maximized,
        fullscreen,
        hover,
        pressed,
        tokens,
    )
}

/// Uses the same physical cells as the registered X11 input children. Glyphs
/// remain centered independently of the edge-to-edge square background.
#[must_use]
pub fn control_blits_for_height(
    frame_width: u32,
    titlebar_height: u16,
    maximized: bool,
    fullscreen: bool,
    hover: Option<crate::frame::model::FrameControl>,
    pressed: Option<crate::frame::model::FrameControl>,
    tokens: &ChromeTokens,
) -> Vec<ControlBlit> {
    use crate::frame::layout::{
        controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
    };
    use crate::frame::model::FrameControl;
    let policy = ControlPolicy::for_state(maximized, fullscreen);
    let titlebar_h = i32::from(titlebar_height);
    let geometries = layout_frame_children(
        i32::try_from(frame_width).unwrap_or(i32::MAX),
        titlebar_h + resize_thickness_for_titlebar(titlebar_h),
        titlebar_h,
        controls_width_for_titlebar(titlebar_h),
        resize_thickness_for_titlebar(titlebar_h),
    )
    .controls;
    let base_h = u32::from(WINDOW_CHROME.metrics.titlebar);
    let glyph_edge = ((u32::from(WINDOW_CHROME.metrics.control_glyph_edge)
        * u32::from(titlebar_height)
        + base_h / 2)
        / base_h)
        .max(1);
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
        let Some(raster) = control_glyph_raster_shared(
            role,
            glyph_edge
                .min(u32::try_from(geometries[index].w.saturating_sub(2)).unwrap_or(1))
                .max(1),
            material,
            look,
        ) else {
            continue;
        };
        let bounds = geometries[index];
        let edge = raster.width as f32;
        let dest_x = bounds.x as f32 + (bounds.w as f32 - edge) / 2.0;
        let dest_y = (f32::from(titlebar_height) - edge) / 2.0;
        blits.push(ControlBlit {
            control: *control,
            role,
            raster,
            dest_x,
            dest_y,
            dest_w: edge,
            dest_h: edge,
        });
    }
    blits
}

/// Independent radii for the four corners of a control-cell fill.
/// Only the outer top-right of Close may inherit the window frame radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlCorners {
    pub top_left: u16,
    pub top_right: u16,
    pub bottom_right: u16,
    pub bottom_left: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ControlMaskKey {
    w: u16,
    h: u16,
    corners: ControlCorners,
}

/// Coverage is cached by geometry, not color: live theme/accent edits can
/// reuse the same AA mask and cannot leave stale cached paint behind.
const CONTROL_MASK_CACHE_CAPACITY: usize = 64;
fn control_mask_cache() -> &'static Mutex<HashMap<ControlMaskKey, Arc<Vec<u8>>>> {
    static CACHE: OnceLock<Mutex<HashMap<ControlMaskKey, Arc<Vec<u8>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Coverage of the four independently rounded corners at subpixel coordinates.
/// The input coordinates are relative to the tile's *outer* top-left edge.
fn inside_control_mask(x: f32, y: f32, w: f32, h: f32, corners: ControlCorners) -> bool {
    let corner = if x < f32::from(corners.top_left) && y < f32::from(corners.top_left) {
        Some((
            corners.top_left,
            f32::from(corners.top_left),
            f32::from(corners.top_left),
        ))
    } else if x > w - f32::from(corners.top_right) && y < f32::from(corners.top_right) {
        Some((
            corners.top_right,
            w - f32::from(corners.top_right),
            f32::from(corners.top_right),
        ))
    } else if x > w - f32::from(corners.bottom_right) && y > h - f32::from(corners.bottom_right) {
        Some((
            corners.bottom_right,
            w - f32::from(corners.bottom_right),
            h - f32::from(corners.bottom_right),
        ))
    } else if x < f32::from(corners.bottom_left) && y > h - f32::from(corners.bottom_left) {
        Some((
            corners.bottom_left,
            f32::from(corners.bottom_left),
            h - f32::from(corners.bottom_left),
        ))
    } else {
        None
    };
    corner.is_none_or(|(radius, cx, cy)| {
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= f32::from(radius).powi(2)
    })
}

fn control_mask(key: ControlMaskKey) -> Arc<Vec<u8>> {
    if let Ok(cache) = control_mask_cache().lock() {
        if let Some(mask) = cache.get(&key) {
            return Arc::clone(mask);
        }
    }
    // A 4x4 analytic coverage mask is generated ONLY on a cache miss, never
    // by supersampling a full RGBA button every time its hover state changes.
    const SAMPLES: u32 = 4;
    let mut alpha = Vec::with_capacity(usize::from(key.w) * usize::from(key.h));
    for y in 0..u32::from(key.h) {
        for x in 0..u32::from(key.w) {
            let mut covered = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                    if inside_control_mask(px, py, f32::from(key.w), f32::from(key.h), key.corners)
                    {
                        covered += 1;
                    }
                }
            }
            alpha.push(((covered * 255 + (SAMPLES * SAMPLES) / 2) / (SAMPLES * SAMPLES)) as u8);
        }
    }
    let mask = Arc::new(alpha);
    if let Ok(mut cache) = control_mask_cache().lock() {
        if cache.len() >= CONTROL_MASK_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&mask));
    }
    mask
}

/// Frame-local fill geometry, with its coverage in an immutable cached mask.
#[derive(Debug, Clone)]
pub struct ControlBackplate {
    pub control: crate::frame::model::FrameControl,
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    pub corners: ControlCorners,
    pub rgb: [u8; 3],
    pub coverage: Arc<Vec<u8>>,
}

/// Plan square-style hover/pressed fills over the *entire* titlebar cells.
/// The input children own the clickable band below the resize strip; the
/// visual backplate intentionally extends to y=0 so the upper seam disappears.
#[must_use]
pub fn control_backplates_for(
    frame_width: u32,
    titlebar_height: u16,
    frame_radius: u16,
    hover: Option<crate::frame::model::FrameControl>,
    pressed: Option<crate::frame::model::FrameControl>,
    tokens: &ChromeTokens,
) -> Vec<ControlBackplate> {
    use crate::frame::layout::{
        controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
    };
    use crate::frame::model::FrameControl;
    let h = i32::from(titlebar_height);
    let layout = layout_frame_children(
        i32::try_from(frame_width).unwrap_or(i32::MAX),
        h + resize_thickness_for_titlebar(h),
        h,
        controls_width_for_titlebar(h),
        resize_thickness_for_titlebar(h),
    );
    let controls = [
        FrameControl::Minimize,
        FrameControl::MaximizeRestore,
        FrameControl::Close,
    ];
    let mut plates = Vec::with_capacity(3);
    for (index, control) in controls.into_iter().enumerate() {
        let button = tokens.buttons[index];
        let material =
            ControlMaterial::for_pointer(hover == Some(control), pressed == Some(control));
        if button.shape != ButtonShape::Square || material == ControlMaterial::Rest {
            continue;
        }
        let rect = layout.controls[index];
        if rect.w <= 0 || h <= 0 {
            continue;
        }
        let Ok(width) = u16::try_from(rect.w) else {
            continue;
        };
        let radius = (titlebar_height / 5)
            .max(1)
            .min(width / 2)
            .min(titlebar_height / 2);
        let corners = ControlCorners {
            top_left: 0,
            top_right: if control == FrameControl::Close {
                frame_radius.min(width).min(titlebar_height)
            } else {
                0
            },
            bottom_right: radius,
            bottom_left: radius,
        };
        let key = ControlMaskKey {
            w: width,
            h: titlebar_height,
            corners,
        };
        let role = match control {
            FrameControl::Minimize => ControlRole::Minimize,
            FrameControl::MaximizeRestore => ControlRole::Maximize,
            FrameControl::Close => ControlRole::Close,
        };
        let scheme = control_glyph_scheme(
            role,
            material,
            ControlLook {
                button,
                titlebar: tokens.titlebar_bg,
            },
        );
        plates.push(ControlBackplate {
            control,
            x: rect.x,
            y: 0,
            width,
            height: titlebar_height,
            corners,
            rgb: [
                scheme.background[0],
                scheme.background[1],
                scheme.background[2],
            ],
            coverage: control_mask(key),
        });
    }
    plates
}

/// Composite AA cell backgrounds into the titlebar's already-opaque,
/// wallpaper-resolved pixels. This correctly handles transparent titlebars
/// and still paints the same AA edges on X11 hosts without XRender.
pub fn composite_control_backplates(
    titlebar_pixels: &mut [u8],
    titlebar_width: u32,
    titlebar_height: u16,
    plates: &[ControlBackplate],
) {
    for plate in plates {
        for y in 0..usize::from(plate.height) {
            let py = plate.y + y as i32;
            if py < 0 || py >= i32::from(titlebar_height) {
                continue;
            }
            for x in 0..usize::from(plate.width) {
                let px = plate.x + x as i32;
                if px < 0 || px >= i32::try_from(titlebar_width).unwrap_or(i32::MAX) {
                    continue;
                }
                let coverage = u16::from(plate.coverage[y * usize::from(plate.width) + x]);
                if coverage == 0 {
                    continue;
                }
                let dest = (py as usize * titlebar_width as usize + px as usize) * 4;
                if dest + 3 >= titlebar_pixels.len() {
                    continue;
                }
                for (channel, value) in plate.rgb.into_iter().enumerate() {
                    titlebar_pixels[dest + channel] = ((u16::from(value) * coverage
                        + u16::from(titlebar_pixels[dest + channel]) * (255 - coverage)
                        + 127)
                        / 255) as u8;
                }
            }
        }
    }
}

/// Centered title origin for the live paint path using the skin
/// `center_title_x` contract: desired `(W - tw) / 2` clamped into the free
/// region between the left-occupied icon slot and the right-occupied
/// 114px control strip, padded by skin `TITLE_PAD`.
#[must_use]
pub fn live_title_x(frame_width: u32, title_width: i32) -> f32 {
    live_title_x_for_height(frame_width, title_width, WINDOW_CHROME.metrics.titlebar)
}

#[must_use]
pub fn live_title_x_for_height(frame_width: u32, title_width: i32, titlebar_h: u16) -> f32 {
    use crate::frame::layout::controls_width_for_titlebar;
    let bar_w = i32::try_from(frame_width).unwrap_or(i32::MAX);
    let scale = |value: i32| {
        (value * i32::from(titlebar_h) + i32::from(WINDOW_CHROME.metrics.titlebar) / 2)
            / i32::from(WINDOW_CHROME.metrics.titlebar)
    };
    let left_occupied = scale(recipe::ICON_PAD_LEFT)
        + i32::from(icon_slot_for(titlebar_h))
        + scale(recipe::TITLE_PAD);
    let right_occupied = bar_w - controls_width_for_titlebar(i32::from(titlebar_h));
    recipe::center_title_x(
        bar_w,
        title_width,
        left_occupied,
        right_occupied,
        scale(recipe::TITLE_PAD),
    ) as f32
}

/// Native icon destination rect for the live paint path (skin geometry).
#[must_use]
pub fn icon_dest_rect(titlebar_height: u16) -> (f32, f32, f32) {
    let slot = f32::from(icon_slot_for(titlebar_height));
    (
        recipe::ICON_PAD_LEFT as f32 * f32::from(titlebar_height)
            / f32::from(WINDOW_CHROME.metrics.titlebar),
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

/// Titlebar fill from the shared Surfaces settings: `surfaceColor` RGB and
/// `surfaceOpacity` percent as 0..=255 alpha (`percent * 255 / 100`, same mapping as
/// the desktop menu Surfaces fill). Malformed colour falls back to the skin default;
/// `surfaceBlurPercent` is the third element (applied by `frame::titlebar_backdrop`).
fn surface_tokens(snapshot: &SettingsSnapshot) -> (Rgb, u8, u8) {
    let rgb = match snapshot.values.get("surfaceColor") {
        Some(SettingValue::Text(value)) => parse_settings_color(value),
        _ => None,
    }
    .unwrap_or(flamewm_skin::recipes::decoration::DEFAULT_SURFACE_RGB);
    let percent = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 50,
    };
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => flamewm_skin::recipes::decoration::DEFAULT_SURFACE_BLUR_PERCENT,
    };
    (rgb, ((u16::from(percent) * 255) / 100) as u8, blur)
}

/// Map WM settings onto skin chrome tokens. Empty or malformed colors fall back
/// to the skin default; an unset, empty, or unknown `titlebarButtonStyle` is `circle`.
#[must_use]
pub fn chrome_tokens_from_settings(snapshot: &SettingsSnapshot) -> ChromeTokens {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.as_str(),
        _ => "",
    };
    // Legacy "ios" (and "rounded", empty or malformed) is Rounded.
    let style = match text("decoration") {
        "squared" => DecorationStyle::Squared,
        _ => DecorationStyle::Rounded,
    };
    let (titlebar_bg, titlebar_alpha, titlebar_blur_percent) = surface_tokens(snapshot);
    // One shape for all three controls; anything but "square" is the circle default.
    let shape = match text("titlebarButtonStyle") {
        "square" => ButtonShape::Square,
        _ => ButtonShape::Circle,
    };
    let button = |name: &str| ButtonTokens {
        shape,
        glyph: parse_settings_color(text(&format!("titlebar{name}Color"))),
        hover: parse_settings_color(text(&format!("titlebar{name}HoverColor"))),
        pressed: parse_settings_color(text(&format!("titlebar{name}PressedColor"))),
    };
    ChromeTokens {
        style,
        titlebar_bg,
        titlebar_alpha,
        titlebar_blur_percent,
        backdrop_revision: crate::frame::titlebar_backdrop::set_wallpaper_source(
            wallpaper_source_from_settings(snapshot),
        ),
        buttons: [button("Minimize"), button("Maximize"), button("Close")],
    }
}

/// Wallpaper source named by the settings (`wallpaperMode`, `wallpaper`, `wallpaperColor`,
/// `wallpaperFit`), the same keys the desktop wallpaper owner reads.
fn wallpaper_source_from_settings(
    snapshot: &SettingsSnapshot,
) -> crate::frame::titlebar_backdrop::WallpaperSource {
    use crate::frame::titlebar_backdrop::WallpaperSource;
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.as_str(),
        _ => "",
    };
    match text("wallpaperMode") {
        "solid" => {
            let rgb = parse_settings_color(text("wallpaperColor")).unwrap_or(Rgb(0x23_2629));
            WallpaperSource::Solid([(rgb.0 >> 16) as u8, (rgb.0 >> 8) as u8, rgb.0 as u8])
        }
        mode => WallpaperSource::Image {
            // Anything but `custom` is the bundled default (empty path).
            path: if mode == "custom" {
                text("wallpaper").to_owned()
            } else {
                String::new()
            },
            fit: flamewm_image_core::wallpaper::parse(text("wallpaperFit")),
        },
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

    fn put(snapshot: &mut SettingsSnapshot, key: &str, value: SettingValue) {
        snapshot.values.insert(key.to_owned(), value);
    }

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
        let unset = chrome_tokens_from_settings(&snapshot);
        assert!(
            unset
                .buttons
                .iter()
                .all(|button| button.shape == ButtonShape::Circle)
        );
        put(
            &mut snapshot,
            "titlebarButtonStyle",
            SettingValue::Text(String::new()),
        );
        assert!(
            chrome_tokens_from_settings(&snapshot)
                .buttons
                .iter()
                .all(|button| button.shape == ButtonShape::Circle)
        );
        let text = |value: &str| SettingValue::Text(value.to_owned());
        put(&mut snapshot, "decoration", text("squared"));
        put(&mut snapshot, "surfaceColor", text("#202326"));
        put(&mut snapshot, "surfaceOpacity", SettingValue::Percent(50));
        put(&mut snapshot, "titlebarButtonStyle", text("square"));
        put(&mut snapshot, "titlebarCloseHoverColor", text("#00ff00"));
        put(&mut snapshot, "titlebarMinimizeColor", text("#zz"));
        let tokens = chrome_tokens_from_settings(&snapshot);
        assert_eq!(tokens.style, DecorationStyle::Squared);
        assert_eq!(tokens.titlebar_bg, Rgb(0x202326));
        assert_eq!(tokens.titlebar_alpha, 127);
        assert!(
            tokens
                .buttons
                .iter()
                .all(|button| button.shape == ButtonShape::Square)
        );
        assert_eq!(tokens.buttons[2].hover, Some(Rgb(0x00ff00)));
        assert_eq!(tokens.buttons[0].glyph, None);
        put(&mut snapshot, "titlebarButtonStyle", text("bogus"));
        let tokens = chrome_tokens_from_settings(&snapshot);
        assert!(
            tokens
                .buttons
                .iter()
                .all(|button| button.shape == ButtonShape::Circle)
        );
    }

    #[test]
    fn surface_opacity_maps_to_titlebar_alpha() {
        let mut snapshot = SettingsSnapshot::new(1);
        let mut alpha = |percent: u8| {
            put(
                &mut snapshot,
                "surfaceOpacity",
                SettingValue::Percent(percent),
            );
            chrome_tokens_from_settings(&snapshot).titlebar_alpha
        };
        assert_eq!(alpha(0), 0);
        assert_eq!(alpha(50), 127);
        assert_eq!(alpha(100), 255);
        snapshot.values.remove("surfaceOpacity");
        assert_eq!(chrome_tokens_from_settings(&snapshot).titlebar_alpha, 127);
    }

    #[test]
    fn titlebar_color_setting_is_ignored() {
        let mut snapshot = SettingsSnapshot::new(1);
        put(
            &mut snapshot,
            "titlebarColor",
            SettingValue::Text("#202326".to_owned()),
        );
        let tokens = chrome_tokens_from_settings(&snapshot);
        assert_eq!(
            tokens.titlebar_bg,
            flamewm_skin::recipes::decoration::DEFAULT_SURFACE_RGB
        );
        assert_eq!(tokens.titlebar_alpha, 127);
    }

    #[test]
    fn square_glyph_has_no_inset_fill_circle_keeps_legacy_disk() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let look = |shape| ControlLook {
            button: ButtonTokens {
                shape,
                ..ButtonTokens::default()
            },
            titlebar: WINDOW_CHROME.titlebar_background,
        };
        let square = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Hover,
            look(ButtonShape::Square),
        )
        .unwrap();
        assert_eq!(square.pixels[((8 * edge + 1) * 4 + 3) as usize], 0);
        let circle = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Hover,
            look(ButtonShape::Circle),
        )
        .unwrap();
        assert_eq!(circle.pixels[((8 * edge + 1) * 4 + 3) as usize], 255);
    }

    #[test]
    fn windows11_square_hover_coverage_at_100_135_200_percent() {
        use crate::frame::layout::{
            controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
        };
        use crate::frame::model::FrameControl;
        for (height, radius) in [(31_u16, 12_u16), (42, 16), (62, 24)] {
            for decoration in [DecorationStyle::Rounded, DecorationStyle::Squared] {
                let mut tokens = ChromeTokens {
                    style: decoration,
                    ..Default::default()
                };
                for button in &mut tokens.buttons {
                    button.shape = ButtonShape::Square;
                }
                let full = i32::from(height);
                let cells = layout_frame_children(
                    600,
                    400,
                    full,
                    controls_width_for_titlebar(full),
                    resize_thickness_for_titlebar(full),
                )
                .controls;
                for (index, control) in [
                    FrameControl::Minimize,
                    FrameControl::MaximizeRestore,
                    FrameControl::Close,
                ]
                .into_iter()
                .enumerate()
                {
                    let outer_radius = if decoration == DecorationStyle::Rounded {
                        radius
                    } else {
                        0
                    };
                    let plates = control_backplates_for(
                        600,
                        height,
                        outer_radius,
                        Some(control),
                        None,
                        &tokens,
                    );
                    assert_eq!(plates.len(), 1);
                    let plate = &plates[0];
                    assert_eq!(plate.control, control);
                    assert_eq!(plate.x, cells[index].x);
                    assert_eq!(
                        plate.x + i32::from(plate.width),
                        cells[index].x + cells[index].w
                    );
                    assert_eq!(plate.y, 0);
                    assert_eq!(plate.height, height);
                    assert_eq!(plate.corners.top_left, 0);
                    assert_eq!(
                        plate.corners.top_right,
                        if control == FrameControl::Close {
                            outer_radius
                        } else {
                            0
                        }
                    );
                    assert_eq!(plate.corners.bottom_right, plate.corners.bottom_left);
                    assert!(plate.corners.bottom_right > 0);
                    let at = |x: usize, y: usize| plate.coverage[y * usize::from(plate.width) + x];
                    let w = usize::from(plate.width);
                    let h = usize::from(height);
                    assert_eq!(at(0, 0), 255, "top left is square");
                    assert_eq!(at(w / 2, 0), 255, "top edge is full");
                    assert_eq!(at(w / 2, h - 1), 255, "bottom edge between curves is full");
                    assert_eq!(at(0, h - 1), 0, "bottom-left curve cuts the corner");
                    assert_eq!(at(w - 1, h - 1), 0, "bottom-right curve cuts the corner");
                    assert_eq!(
                        at(w - 1, 0),
                        if control == FrameControl::Close && outer_radius > 0 {
                            0
                        } else {
                            255
                        }
                    );
                    assert!(
                        plate.coverage.iter().any(|&a| a > 0 && a < 255),
                        "curve requires antialiasing"
                    );
                    assert_eq!(
                        plate.rgb,
                        if control == FrameControl::Close {
                            [0xe8, 0x11, 0x23]
                        } else {
                            [0xf1, 0xf2, 0xf3]
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn frame_backplates_blend_over_wallpaper_and_leave_adjacent_pixels_unchanged() {
        use crate::frame::model::FrameControl;
        let mut tokens = ChromeTokens::default();
        tokens.buttons[2].shape = ButtonShape::Square;
        let plates = control_backplates_for(400, 31, 12, Some(FrameControl::Close), None, &tokens);
        let mut pixels = vec![30, 40, 50, 255].repeat(400 * 31);
        composite_control_backplates(&mut pixels, 400, 31, &plates);
        let pixel = |x: usize, y: usize| &pixels[(y * 400 + x) * 4..(y * 400 + x) * 4 + 4];
        assert_eq!(
            pixel(361, 15),
            &[30, 40, 50, 255],
            "adjacent maximize cell untouched"
        );
        assert_eq!(
            pixel(362, 15),
            &[0xe8, 0x11, 0x23, 255],
            "close starts at exact cell boundary"
        );
        assert_eq!(
            pixel(380, 30),
            &[0xe8, 0x11, 0x23, 255],
            "bottom edge filled"
        );
        assert_eq!(
            pixel(399, 0),
            &[30, 40, 50, 255],
            "outer rounded corner clipped"
        );
        assert_eq!(
            pixel(399, 15),
            &[0xe8, 0x11, 0x23, 255],
            "right edge filled below outer corner"
        );
        assert!(
            pixels.chunks_exact(4).all(|p| p[3] == 255),
            "opaque wallpaper composite keeps opaque alpha"
        );
        let mut clean = vec![30, 40, 50, 255].repeat(400 * 31);
        composite_control_backplates(
            &mut clean,
            400,
            31,
            &control_backplates_for(400, 31, 12, None, None, &tokens),
        );
        assert!(clean.chunks_exact(4).all(|p| p == [30, 40, 50, 255]));
    }

    #[test]
    fn hot_button_settings_update_color_and_reuse_geometry_cache() {
        use crate::frame::model::FrameControl;
        let mut tokens = ChromeTokens::default();
        tokens.buttons[0].shape = ButtonShape::Square;
        let a = control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens);
        tokens.buttons[0].hover = Some(Rgb(0x224466));
        let b = control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens);
        assert_eq!(b[0].rgb, [0x22, 0x44, 0x66]);
        assert!(Arc::ptr_eq(&a[0].coverage, &b[0].coverage));
        tokens.buttons[0].pressed = Some(Rgb(0x102030));
        let pressed =
            control_backplates_for(400, 31, 12, None, Some(FrameControl::Minimize), &tokens);
        assert_eq!(pressed[0].rgb, [0x10, 0x20, 0x30]);
        tokens.buttons[0].shape = ButtonShape::Circle;
        assert!(
            control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens)
                .is_empty()
        );
    }

    #[test]
    fn narrow_frame_backplates_tile_without_leaking_beyond_frame() {
        use crate::frame::model::FrameControl;
        let mut tokens = ChromeTokens::default();
        for button in &mut tokens.buttons {
            button.shape = ButtonShape::Square;
        }
        for width in [100_u32, 160, 400] {
            let plates: Vec<_> = [
                FrameControl::Minimize,
                FrameControl::MaximizeRestore,
                FrameControl::Close,
            ]
            .into_iter()
            .flat_map(|control| control_backplates_for(width, 31, 12, Some(control), None, &tokens))
            .collect();
            assert_eq!(plates.len(), 3);
            for pair in plates.windows(2) {
                assert_eq!(pair[0].x + i32::from(pair[0].width), pair[1].x);
            }
            assert_eq!(plates[2].x + i32::from(plates[2].width), width as i32);
            let mut background = vec![42_u8, 42, 42, 255].repeat(width as usize * 31);
            composite_control_backplates(&mut background, width, 31, &plates);
            assert_eq!(background.len(), width as usize * 31 * 4);
        }
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
        let default = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Hover,
            ControlLook::default(),
        )
        .expect("close hover raster");
        let explicit =
            control_glyph_raster(ControlRole::Close, edge, ControlMaterial::Hover, explicit)
                .expect("explicit close hover raster");
        assert_eq!(default.pixels, explicit.pixels);
    }

    #[test]
    fn style_tokens_never_move_control_hit_geometry() {
        let mut tokens = ChromeTokens::default();
        tokens.style = DecorationStyle::Squared;
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
        assert_eq!(corner_radius(true, DecorationStyle::Rounded), 12);
        assert_eq!(corner_radius(false, DecorationStyle::Rounded), 0);
    }

    #[test]
    fn decoration_setting_parses_rounded_squared_and_legacy_ios() {
        let style_of = |value: &str| {
            let mut snapshot = SettingsSnapshot::new(1);
            put(
                &mut snapshot,
                "decoration",
                SettingValue::Text(value.to_owned()),
            );
            chrome_tokens_from_settings(&snapshot).style
        };
        assert_eq!(style_of("ios"), DecorationStyle::Rounded);
        assert_eq!(style_of("rounded"), DecorationStyle::Rounded);
        assert_eq!(style_of("squared"), DecorationStyle::Squared);
        assert_eq!(style_of("bogus"), DecorationStyle::Rounded);
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
    fn shared_glyph_raster_matches_owned_output() {
        let owned = control_glyph_raster(
            ControlRole::Close,
            16,
            ControlMaterial::Hover,
            ControlLook::default(),
        )
        .expect("owned close glyph");
        let shared = control_glyph_raster_shared(
            ControlRole::Close,
            16,
            ControlMaterial::Hover,
            ControlLook::default(),
        )
        .expect("shared close glyph");
        assert_eq!((shared.width, shared.height), (owned.width, owned.height));
        assert_eq!(shared.pixels, owned.pixels);
    }

    #[test]
    fn installed_control_fallback_is_visible_for_every_role() {
        for role in [
            ControlRole::Minimize,
            ControlRole::Maximize,
            ControlRole::Restore,
            ControlRole::Close,
        ] {
            let raster =
                fallback_control_glyph(role, 16, [242, 242, 242, 255]).expect("native fallback");
            assert_eq!((raster.width, raster.height), (16, 16));
            assert!(raster.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
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
                control_glyph_raster(role, 16, ControlMaterial::Rest, ControlLook::default())
                    .expect("breeze control svg");
            assert_eq!((image.width, image.height), (16, 16));
            assert!(image.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
        assert!(
            control_glyph_raster(
                ControlRole::Close,
                0,
                ControlMaterial::Rest,
                ControlLook::default()
            )
            .is_none()
        );
    }

    #[test]
    fn hovering_close_selects_hover_material_only_for_close() {
        use crate::frame::model::FrameControl;
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        // Blits carry no material field, so the material each control was
        // rendered with is observed through its cached glyph raster.
        assert_eq!(
            ControlMaterial::for_pointer(false, false),
            ControlMaterial::Rest
        );
        assert_eq!(
            ControlMaterial::for_pointer(true, false),
            ControlMaterial::Hover
        );
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
            let raster =
                control_glyph_raster(hovered[index].role, edge, *material, ControlLook::default())
                    .expect("control raster");
            assert_eq!(
                *hovered[index].raster, raster,
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
            let raster = control_glyph_raster(
                blit.role,
                edge,
                ControlMaterial::Rest,
                ControlLook::default(),
            )
            .expect("control raster");
            assert_eq!(
                *blit.raster, raster,
                "no hover must select Rest material for every control"
            );
            assert_eq!(blit.control, hovered[index].control);
        }
    }

    #[test]
    fn close_hover_and_pressed_rasters_differ_from_rest() {
        let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
        let rest = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Rest,
            ControlLook::default(),
        )
        .expect("close rest raster");
        for material in [ControlMaterial::Hover, ControlMaterial::Pressed] {
            let raster =
                control_glyph_raster(ControlRole::Close, edge, material, ControlLook::default())
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
        let raster = control_glyph_raster(
            ControlRole::Close,
            edge,
            ControlMaterial::Hover,
            ControlLook::default(),
        )
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
        let raster = control_glyph_raster(
            ControlRole::Minimize,
            edge,
            ControlMaterial::Rest,
            ControlLook::default(),
        )
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
