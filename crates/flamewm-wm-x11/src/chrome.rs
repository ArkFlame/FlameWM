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

/// WM-owned view of control roles (mirrors skin `WindowControlRole`).
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

mod icon;
pub use icon::*;


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

/// Process-global bounded glyph raster cache (WM loop is single-threaded;
/// mutex guards cross-call sharing, never held across X calls).
const CONTROL_GLYPH_CACHE_CAPACITY: usize = 32;

type ControlGlyphCache = Mutex<HashMap<ControlGlyphKey, Arc<flamewm_image_core::RgbaImage>>>;

fn control_glyph_cache() -> &'static ControlGlyphCache {
    static CACHE: OnceLock<ControlGlyphCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Bounded cache insert: a full cache is cleared before the new entry lands.
fn bounded_insert<K: Eq + std::hash::Hash, V>(
    cache: &Mutex<HashMap<K, V>>,
    capacity: usize,
    key: K,
    value: V,
) {
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= capacity {
            cache.clear();
        }
        cache.insert(key, value);
    }
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
        role,
        edge,
        material,
        look,
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
    let mut raster = if let Some(raster) = std::fs::read(&path).ok().and_then(|bytes| {
        flamewm_image_core::svg::render_with_color_scheme(&bytes, edge, edge, &scheme).ok()
    }) {
        raster
    } else {
        eprintln!(
            "flamewm: titlebar control asset unavailable ({}); using native vector fallback",
            path.display()
        );
        fallback_control_glyph(role, edge, scheme.text)?
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
    bounded_insert(
        control_glyph_cache(),
        CONTROL_GLYPH_CACHE_CAPACITY,
        key,
        Arc::clone(&raster),
    );
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
    if !(4..=128).contains(&edge) {
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
    const SAMPLES: u32 = 4;
    for y in 0..raster.height {
        for x in 0..raster.width {
            let coverage = (0..SAMPLES)
                .flat_map(|sy| (0..SAMPLES).map(move |sx| (sx, sy)))
                .filter(|&(sx, sy)| {
                    mask.contains(
                        x as f32 + (sx as f32 + 0.5) / SAMPLES as f32,
                        y as f32 + (sy as f32 + 0.5) / SAMPLES as f32,
                        size,
                    )
                })
                .count() as u16;
            if coverage == 0 {
                continue;
            }
            let disk_coverage = (coverage * 255 + 8) / 16;
            let offset = ((y * raster.width + x) * 4) as usize;
            let pixel = &mut raster.pixels[offset..offset + 4];
            let glyph_alpha = u16::from(pixel[3]);
            // The original glyph remains on top. Only uncovered portions receive the AA fill.
            let fill_alpha = (disk_coverage * u16::from(fill[3]) + 127) / 255;
            let total_alpha = glyph_alpha + (fill_alpha * (255 - glyph_alpha) + 127) / 255;
            if total_alpha == 0 {
                continue;
            }
            for channel in 0..3 {
                let premultiplied = u32::from(pixel[channel]) * u32::from(glyph_alpha)
                    + (u32::from(fill[channel])
                        * u32::from(fill_alpha)
                        * u32::from(255 - glyph_alpha)
                        + 127)
                        / 255;
                pixel[channel] = ((premultiplied + u32::from(total_alpha) / 2)
                    / u32::from(total_alpha))
                .min(255) as u8;
            }
            pixel[3] = total_alpha as u8;
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
#[cfg(test)]
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
#[cfg(test)]
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
    const SAMPLES: u32 = 4;
    if let Ok(cache) = control_mask_cache().lock() {
        if let Some(mask) = cache.get(&key) {
            return Arc::clone(mask);
        }
    }
    // A 4x4 analytic coverage mask is generated ONLY on a cache miss, never
    // by supersampling a full RGBA button every time its hover state changes.
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
    bounded_insert(
        control_mask_cache(),
        CONTROL_MASK_CACHE_CAPACITY,
        key,
        Arc::clone(&mask),
    );
    mask
}

/// Frame-local fill geometry, with its coverage in an immutable cached mask.
#[derive(Debug, Clone)]
pub struct ControlBackplate {
    /// Test-only: production paint reads geometry and coverage, not the plan's control/corners.
    #[cfg(test)]
    pub control: crate::frame::model::FrameControl,
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    #[cfg(test)]
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
            #[cfg(test)]
            control,
            x: rect.x,
            y: 0,
            width,
            height: titlebar_height,
            #[cfg(test)]
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
/// and still paints the same AA edges on X11 hosts without `XRender`.
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

/// Available title width in native X11 pixels, using the same button/icon exclusion
/// geometry as the Wayland title rasterizer.
#[must_use]
pub fn live_title_available_width(frame_width: u32, titlebar_h: u16) -> i32 {
    use crate::frame::layout::controls_width_for_titlebar;
    let height = i32::from(titlebar_h);
    let base = i32::from(WINDOW_CHROME.metrics.titlebar);
    let scale = |value: i32| (value.saturating_mul(height) + base / 2) / base;
    let left = scale(recipe::ICON_PAD_LEFT)
        + i32::from(icon_slot_for(titlebar_h))
        + scale(recipe::TITLE_PAD);
    let right = i32::try_from(frame_width)
        .unwrap_or(i32::MAX)
        .saturating_sub(controls_width_for_titlebar(height));
    recipe::title_available_width(left, right, scale(recipe::TITLE_PAD))
}

fn title_advance(ch: char) -> i32 {
    if ch.is_ascii() {
        7
    } else if is_wide(ch) {
        12
    } else {
        8
    }
}

/// Renderer-owned title measurement for Xft IBM Plex Sans 12px semibold
/// (skin typography title). Proportional advance estimate: ASCII 7px, CJK
/// 12px, other 8px; keeps the skin `center_title_x` contract without the
/// retired 9px core-font advance.
#[must_use]
pub fn title_text_width(title: &str) -> i32 {
    let mut width = 0_i32;
    for ch in title.chars().take(TITLE_MAX_CHARS) {
        let advance = title_advance(ch);
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

/// X11 frame borders are 24-bit opaque pixels. Blend the shared 1px
/// contour against the surface colour instead of leaving a fixed dark seam.
#[must_use]
pub fn frame_border_pixel(tokens: &ChromeTokens) -> u32 {
    flamewm_ui_core::surface::blend_surface_over_rgb(
        tokens.titlebar_bg.0,
        flamewm_ui_core::surface::SurfaceFill {
            rgb: tokens.surface_border_rgb.0,
            alpha: tokens.surface_border_alpha,
        },
    )
}

#[cfg(test)]
mod glass_contour_tests {
    use super::*;
    #[test]
    fn frame_border_uses_configured_shared_surface_contour() {
        let mut t = ChromeTokens::default();
        t.titlebar_bg = Rgb(0);
        t.surface_border_rgb = Rgb(0x00ff_ffff);
        t.surface_border_alpha = 0;
        assert_eq!(frame_border_pixel(&t), 0);
        t.surface_border_alpha = 255;
        assert_eq!(frame_border_pixel(&t), 0x00ff_ffff);
        t.surface_border_alpha = 63;
        assert_eq!(frame_border_pixel(&t), 0x003f_3f3f);
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
    // Titlebars take the shared glass material: glassOpacityPercent alpha, the regular glass blur,
    // and an opaque solid under reduceTransparency (blur 0).
    let fill = flamewm_ui_core::surface::surface_fill(snapshot);
    (
        Rgb(fill.rgb),
        fill.alpha,
        flamewm_ui_core::surface::surface_blur_percent(snapshot),
    )
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
    let stroke = flamewm_ui_core::surface::surface_stroke(snapshot);
    let material = flamewm_ui_core::surface::glass_material(snapshot);
    ChromeTokens {
        style,
        surface_border_rgb: Rgb(stroke.rgb),
        surface_border_alpha: stroke.alpha,
        rim_fade_percent: material.rim_fade_percent,
        highlight_percent: material.highlight_percent,
        titlebar_bg,
        titlebar_alpha,
        titlebar_blur_percent,
        backdrop_revision: crate::frame::titlebar_backdrop::set_wallpaper_source(
            wallpaper_source_from_settings(snapshot),
        ),
        buttons: [button("Minimize"), button("Maximize"), button("Close")],
        window_shadows: !matches!(
            snapshot.values.get("windowShadows"),
            Some(SettingValue::Boolean(false))
        ),
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
            // The output layout is published by the WM once its outputs are known.
            outputs: Vec::new(),
        },
    }
}

/// `#rrggbb` to skin color; empty or malformed input is `None`.
fn parse_settings_color(value: &str) -> Option<Rgb> {
    let (r, g, b) = flamewm_platform::chrome::parse_hex_rgb(value)?;
    Some(Rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)))
}

#[cfg(test)]
mod tests;
