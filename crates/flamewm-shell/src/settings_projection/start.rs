//! Start menu icon and slide projection.

use super::*;
use flamewm_api::motion::motion_scale;

/// Configured Start icon path (`customStartIcon`). `None` keeps the default `FlameWM` glyph.
#[must_use]
pub fn custom_start_icon(snapshot: &SettingsSnapshot) -> Option<String> {
    let path = text_setting(snapshot, "customStartIcon", "");
    (!path.trim().is_empty()).then_some(path)
}

/// Applies the configured Start icon to the panel. `None`, or a path that fails
/// to load, keeps the packaged default `flamewm-start.svg` already in the template.
pub(super) fn project_start_icon(
    document: &mut impl UiDocumentAccess,
    path: Option<String>,
) -> Result<(), String> {
    // Clearing (or a path that no longer loads) drops any previously applied custom raster.
    match path.as_deref().and_then(|path| load_start_icon(path).ok()) {
        Some(image) => document.image_rgba8(START_ICON_ID, image),
        None => document.image_reset(START_ICON_ID),
    }
}

/// Decodes a custom Start icon to a `START_ICON_SIZE` square RGBA raster through
/// the shared image core, which enforces the format and pixel limits.
pub(super) fn load_start_icon(path: &str) -> Result<RuntimeImage, String> {
    let path = std::path::Path::new(path);
    let format = ImageSourceFormat::from_extension(path)
        .ok_or_else(|| format!("unsupported start icon format: {}", path.display()))?;
    let len = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if len > MAX_START_ICON_BYTES {
        return Err(format!("start icon exceeds {MAX_START_ICON_BYTES} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let image = decode_bytes_at(
        &bytes,
        format,
        START_ICON_SIZE,
        START_ICON_SIZE,
        ScaleFilter::Area,
    )?;
    Ok(RuntimeImage {
        source: path.display().to_string(),
        width: image.width,
        height: image.height,
        pixels: image.pixels,
    })
}

/// Start presentation for the panel edge hosting Start. `start_button` is the Start button's root
/// device rect `(x, y, width, height)`, which Magic Lamp folds into. `None` keeps the immediate
/// show: animations disabled, `none`, an unknown style, or Magic Lamp before the button has layout.
#[must_use]
pub fn start_slide(
    snapshot: &SettingsSnapshot,
    edge: PanelEdge,
    start_button: Option<(i32, i32, u32, u32)>,
) -> Option<SlideConfig> {
    let speed = motion_scale(snapshot);
    if speed == 0
        || !bool_setting(snapshot, "animationsEnabled", true)
        || !bool_setting(snapshot, "startAnimationEnabled", true)
    {
        return None;
    }
    let edge = match edge {
        PanelEdge::Bottom => SlideEdge::Bottom,
        PanelEdge::Top => SlideEdge::Top,
        PanelEdge::Left => SlideEdge::Left,
        PanelEdge::Right => SlideEdge::Right,
    };
    match text_setting(snapshot, "startAnimation", DEFAULT_START_ANIMATION).as_str() {
        "slide" | "slide-up" => Some(SlideConfig::slide(edge, speed)),
        "fade" => Some(SlideConfig::fade(edge, speed)),
        "magic-lamp" => start_button.map(|target| SlideConfig::lamp(edge, speed, target)),
        _ => None,
    }
}
