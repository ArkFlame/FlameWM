use flamewm_api::Point;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_desktop_core::model::DesktopItemKind;
use flamewm_desktop_core::presentation::desktop_label_lines;
use flamewm_integrations_linux::icon_service::{IconJob, IconPriority, IconService};
use flamewm_integrations_linux::icons::{
    IconKey, IconRequest, IconResolver, IconSize, Rgba8Raster,
};
use flamewm_ui_core::context_menu::{menu_surface, surface_fill};
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{UiColor, UiDocumentAccess};

#[cfg(test)]
use crate::entry::icon_names::icon_name;
use crate::entry::icon_names::icon_semantic;
pub use crate::entry::icon_names::{file_icon_name, icon_purpose, launcher_icon_name};
use crate::entry::icon_raster::{
    ICON_TILE_PX, fade_ghost_pixels, flame_fallback_raster, runtime_image,
};
use crate::entry::wallpaper_loader::{WallpaperPresentation, WallpaperRaster};
use crate::entry::wallpaper_stamp::{
    WALLPAPER_STAMP, WallpaperStamp, wallpaper_matches, wallpaper_stamp,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Mirror of the shared icon theme generation for [`ParallelIconService::icon_key`],
/// which stays a static constructor for its existing callers. Written only by
/// [`ParallelIconService::refresh_theme`] on the UI thread.
static THEME_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Current icon theme generation; a change invalidates every projected tile.
pub fn theme_generation() -> u64 {
    THEME_GENERATION.load(Ordering::Acquire)
}

const ICON_KINDS: [&str; 5] = ["directory", "desktop-launcher", "file", "symlink", "trash"];
const LABEL_WIDTH_PX: f32 = 76.0;
const LABEL_FONT_SIZE_PX: f32 = 13.0;
pub const DESKTOP_TILE_WIDTH: f32 = 82.0;
pub const DESKTOP_TILE_HEIGHT: f32 = 82.0;
pub const STICKY_SLOT_COUNT: usize = 16;
fn scaled_icon_px(scale: u16) -> u32 {
    (ICON_TILE_PX * u32::from(scale) + 50) / 100
}

pub fn project_background(
    document: &mut impl UiDocumentAccess,
    presentation: &WallpaperPresentation,
) -> Result<(), String> {
    let has_image = matches!(presentation, WallpaperPresentation::Image(_));
    let (background, watermark) = match presentation {
        WallpaperPresentation::Solid([r, g, b]) => (
            UiColor {
                r: *r,
                g: *g,
                b: *b,
                a: 255,
            },
            false,
        ),
        _ => (
            UiColor {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            true,
        ),
    };
    document.background("desktop", background)?;
    document.visible("desktop-watermark", watermark)?;
    document.visible("desktop-wallpaper-selection", has_image)?;
    let mut stamp = WALLPAPER_STAMP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match presentation {
        WallpaperPresentation::Image(raster) => {
            project_wallpaper_image(document, &mut stamp, raster)?;
        }
        _ => *stamp = None,
    }
    Ok(())
}

pub fn project_global_font_defaults(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    let (size_offset, bold) = global_font_defaults(snapshot);
    document.ui_font_defaults(size_offset, bold)
}

fn global_font_defaults(snapshot: &SettingsSnapshot) -> (f32, bool) {
    let size_offset = match snapshot.values.get("fontSizeOffset") {
        Some(SettingValue::Integer(value)) => (*value).clamp(-2, 4) as f32,
        _ => 0.0,
    };
    let bold = matches!(
        snapshot.values.get("fontBold"),
        Some(SettingValue::Boolean(true))
    );
    (size_offset, bold)
}

/// Sends the image only when it differs from `cache`; the cache advances only
/// after the document accepts the image, so a failed write is retried.
fn project_wallpaper_image(
    document: &mut impl UiDocumentAccess,
    cache: &mut Option<WallpaperStamp>,
    raster: &WallpaperRaster,
) -> Result<(), String> {
    if cache
        .as_ref()
        .is_some_and(|stamp| wallpaper_matches(stamp, raster))
    {
        return Ok(());
    }
    document.image_rgba8_shared(
        "desktop-wallpaper-selection",
        raster.source.clone(),
        raster.width,
        raster.height,
        Arc::clone(&raster.pixels),
    )?;
    *cache = Some(wallpaper_stamp(raster));
    Ok(())
}

/// Desktop context-menu roots whose surface follows the Surfaces settings.
pub const MENU_SURFACE_IDS: [&str; 6] = [
    "desktop-menu",
    "desktop-entry-menu",
    "desktop-convert-wheel",
    "sticky-menu",
    "desktop-confirm",
    "downloads-notice",
];

/// Projects `surfaceColor` / `surfaceOpacity` fill and the `decoration` corner radius
/// (Rounded 12 / Squared 0) onto one desktop menu root. Every menu open, confirm and
/// live settings sync goes through here, so shape and fill stay in step with the shell.
pub fn project_menu_surface(
    document: &mut impl UiDocumentAccess,
    id: &str,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    let fill = surface_fill(snapshot);
    document.background(
        id,
        UiColor {
            r: (fill.rgb >> 16) as u8,
            g: (fill.rgb >> 8) as u8,
            b: fill.rgb as u8,
            a: fill.alpha,
        },
    )?;
    document.corner_radius(
        id,
        if id == "desktop-convert-wheel" {
            164.0
        } else {
            menu_surface(snapshot).radius as f32
        },
    )?;
    // Translucent menus blur the wallpaper beneath them; 0 clears the backdrop.
    document.node_backdrop(id, Some(surface_blur_percent(snapshot)))
}

/// Global `surfaceBlurPercent` (0..=100) that menus and inheriting widgets blur beneath them.
pub(crate) fn surface_blur_percent(snapshot: &SettingsSnapshot) -> u8 {
    flamewm_ui_core::surface::surface_blur_percent(snapshot)
}

/// Clears a desktop menu root's node backdrop (menu hidden).
pub fn clear_menu_backdrop(document: &mut impl UiDocumentAccess, id: &str) -> Result<(), String> {
    document.node_backdrop(id, None)
}

/// Semantic layer assignment, run once at document init. Static ids plus the
/// generated item slots; never called per-frame so visibility math stays raw.
pub fn assign_document_layers(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    document.layer("desktop", UiLayer::SurfaceBackground)?;
    document.layer("desktop-wallpaper-selection", UiLayer::Background)?;
    document.layer("desktop-watermark", UiLayer::Content)?;
    for index in 0..SLOT_COUNT {
        document.layer(&format!("desktop-item-{index}"), UiLayer::Content)?;
    }
    document.layer("desktop-selection", UiLayer::Selection)?;
    document.layer("desktop-drag-ghost", UiLayer::Floating)?;
    document.layer("desktop-convert-preview", UiLayer::Floating)?;
    for slot in 0..STICKY_SLOT_COUNT {
        document.layer(&format!("sticky-{}", slot + 1), UiLayer::Floating)?;
    }
    document.layer("desktop-menu", UiLayer::Popover)?;
    document.layer("desktop-entry-menu", UiLayer::Popover)?;
    document.layer("desktop-convert-wheel", UiLayer::Popover)?;
    document.layer("sticky-menu", UiLayer::Popover)?;
    document.layer("desktop-rename", UiLayer::Popover)?;
    document.layer("desktop-confirm", UiLayer::Modal)?;
    document.layer("downloads-notice", UiLayer::Modal)?;
    Ok(())
}

const SLOT_COUNT: usize = 128;

pub const TRANSIENT_HIDDEN_IDS: [&str; 10] = [
    "desktop-convert-preview",
    "desktop-drag-ghost",
    "desktop-selection",
    "sticky-1",
    "desktop-menu",
    "desktop-entry-menu",
    "desktop-convert-wheel",
    "sticky-menu",
    "desktop-confirm",
    "downloads-notice",
];

/// Runtime-owned first-paint guard: hides selection/sticky/menus/confirm via
/// `hidden_nodes` so no transient node flashes before state sync decides it.
/// Restores authored display on `visible(id, true)`; `hidden_nodes` stays the
/// sole visibility override (authored CSS carries the visible display modes).
pub fn initialize_transient_visibility(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for id in TRANSIENT_HIDDEN_IDS {
        document.visible(id, false)?;
    }
    Ok(())
}

/// Reusable parallel icon service: rows project immediately from the warm
/// cache (fallback = no stale glyph), visible misses submit async HIGH, and
/// completions apply only on exact [`IconKey`] identity match per slot.
pub struct ParallelIconService {
    service: IconService,
}

impl ParallelIconService {
    #[must_use]
    pub fn spawn(packaged_root: PathBuf) -> Self {
        // One generation-scoped index build shared by both workers.
        let base = IconResolver::from_environment(packaged_root, 0);
        let index = std::sync::Arc::clone(base.shared_index());
        let base = std::sync::Arc::new(base);
        let service = IconService::with_index(2, 128, move || base.fork_worker(), index);
        Self { service }
    }

    #[must_use]
    pub fn icon_key(kind: DesktopItemKind, icon_override: Option<&str>) -> IconKey {
        Self::icon_key_scaled(kind, icon_override, 100)
    }

    pub fn icon_key_scaled(
        kind: DesktopItemKind,
        icon_override: Option<&str>,
        scale: u16,
    ) -> IconKey {
        let override_name = icon_override.map(str::trim).filter(|icon| !icon.is_empty());
        IconKey::new(
            icon_purpose(kind, override_name.is_some()),
            IconRequest::name(launcher_icon_name(kind, icon_override)),
            IconSize::new(scaled_icon_px(scale), scaled_icon_px(scale)),
            THEME_GENERATION.load(Ordering::Acquire),
            0,
        )
    }

    /// UI-thread, non-blocking. Requests the `theme` chain off-thread and installs the finished
    /// index. Returns `true` only once the installed index uses that chain, so the caller keeps
    /// retrying each tick until then.
    pub fn apply_icon_theme(&mut self, theme: &str) -> bool {
        let wanted = flamewm_integrations_linux::icons::theme_chain(Some(theme));
        if self.service.shared_index().theme_names == wanted {
            return true;
        }
        self.service.request_theme_change(wanted.clone());
        if let Some(next) = self.service.poll_theme_rebuild() {
            let generation = self.service.install_theme_index(next);
            THEME_GENERATION.store(generation, Ordering::Release);
        }
        self.service.shared_index().theme_names == wanted
    }

    /// UI-thread, non-blocking theme watch. Queues a check on the rebuild
    /// thread (no-op when one is already in flight), then installs any finished
    /// replacement index. Returns true when a new generation was installed.
    /// The mirror takes the generation returned by `install_theme_index`.
    pub fn refresh_theme(&mut self) -> bool {
        self.service.request_theme_rebuild();
        let Some(next) = self.service.poll_theme_rebuild() else {
            return false;
        };
        let generation = self.service.install_theme_index(next);
        THEME_GENERATION.store(generation, Ordering::Release);
        true
    }

    /// Raw FD readable whenever a worker result completes. Register with
    /// the Desktop Reactor; the callback only calls `wake_drain` and the
    /// next reactor tick repaints through the normal flush path.
    #[must_use]
    pub fn wake_fd(&self) -> Option<std::os::unix::io::RawFd> {
        self.service.wake_fd()
    }

    /// Non-blocking drain of pending wake bytes (call after `wake_fd` readable).
    pub fn wake_drain(&mut self) {
        self.service.wake_drain();
    }

    /// Projects row chrome + label immediately; the glyph comes from the warm
    /// cache when present, otherwise the Flame fallback paints synchronously
    /// (never a stale glyph, never blank) and a HIGH request is queued.
    /// Returns the key for slot identity tracking.
    #[allow(clippy::too_many_arguments)]
    pub fn project_row(
        &mut self,
        document: &mut impl UiDocumentAccess,
        index: usize,
        kind: DesktopItemKind,
        label: &str,
        point: Point,
        icon_override: Option<&str>,
        scale: u16,
    ) -> Result<IconKey, String> {
        let item_id = format!("desktop-item-{index}");
        document.visible(&item_id, true)?;
        document.position(&item_id, point.x as f32, point.y as f32)?;
        let factor = f32::from(scale) / 100.0;
        document.size(
            &item_id,
            DESKTOP_TILE_WIDTH * factor,
            DESKTOP_TILE_HEIGHT * factor,
        )?;
        // Keep all click/drag children on the same output-local physical scale as the grid.
        for kind in ICON_KINDS {
            let id = format!("desktop-glyph-{index}-{kind}");
            document.position(&id, 18.0 * factor, 5.0 * factor)?;
            document.size(&id, 44.0 * factor, 44.0 * factor)?;
        }
        let label_id = format!("desktop-label-{index}");
        document.position(&label_id, 2.0 * factor, 52.0 * factor)?;
        document.size(&label_id, 76.0 * factor, 30.0 * factor)?;
        for line in 1..=2 {
            let id = format!("desktop-label-{index}-line-{line}");
            document.size(&id, 76.0 * factor, 15.0 * factor)?;
            document.font_size(&id, LABEL_FONT_SIZE_PX * factor)?;
        }
        // Per-frame theme watch (throttled inside needs_rebuild). A new
        // generation changes the key below, so stale cache entries miss.
        self.refresh_theme();
        let key = Self::icon_key_scaled(kind, icon_override, scale);
        if let Some(Ok(raster)) = self.service.cached(&key) {
            apply_raster(document, index, kind, raster)?;
        } else {
            // Miss: Flame fallback paints synchronously so the tile never
            // shows blank/stale; the worker result replaces it on drain.
            apply_raster(document, index, kind, flame_fallback_raster())?;
            self.request_high(key.clone());
        }
        project_item_label(document, index, label, scale)?;
        Ok(key)
    }

    /// Applies one completed raster only when the slot still expects its key.
    /// Returns false without insert/apply on any identity mismatch.
    pub fn apply_if_current(
        &mut self,
        document: &mut impl UiDocumentAccess,
        index: usize,
        kind: DesktopItemKind,
        expected: &IconKey,
        actual: &IconKey,
        raster: Rgba8Raster,
    ) -> Result<bool, String> {
        if expected != actual {
            return Ok(false);
        }
        apply_raster(document, index, kind, raster)?;
        Ok(true)
    }

    /// Drains completed HIGH jobs; [`IconService`] retains successful rasters
    /// in its bounded shared cache. Callers reproject only slots whose tracked
    /// identity still equals the result key.
    pub fn drain_ready(&mut self) -> Vec<(IconKey, Option<Rgba8Raster>)> {
        self.service
            .drain_ready()
            .into_iter()
            .map(|result| {
                (
                    std::sync::Arc::unwrap_or_clone(result.key),
                    result.result.ok(),
                )
            })
            .collect()
    }

    #[must_use]
    pub fn cached(&self, key: &IconKey) -> Option<Rgba8Raster> {
        self.service.cached(key).and_then(Result::ok)
    }

    pub fn request_high(&mut self, key: IconKey) {
        let _ = self.service.submit(IconJob {
            key,
            priority: IconPriority::High,
        });
    }

    #[must_use]
    pub fn memory_estimate_bytes(&self) -> usize {
        // IconService owns the bounded shared result cache; this facade retains no rasters.
        self.service.result_cache_bytes()
    }
}

fn apply_raster(
    document: &mut impl UiDocumentAccess,
    index: usize,
    kind: DesktopItemKind,
    raster: Rgba8Raster,
) -> Result<(), String> {
    let selected = icon_semantic(kind);
    document.image_rgba8(
        &format!("desktop-glyph-{index}-{selected}"),
        runtime_image(raster),
    )?;
    set_glyph_visibility(document, index, Some(selected))
}

/// Shows only the glyph named by `selected`; `None` hides every glyph of the slot.
fn set_glyph_visibility(
    document: &mut impl UiDocumentAccess,
    index: usize,
    selected: Option<&str>,
) -> Result<(), String> {
    for icon_kind in ICON_KINDS {
        document.visible(
            &format!("desktop-glyph-{index}-{icon_kind}"),
            Some(icon_kind) == selected,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn project_item(
    document: &mut impl UiDocumentAccess,
    icons: &mut ParallelIconService,
    index: usize,
    kind: DesktopItemKind,
    label: &str,
    point: Point,
    icon_override: Option<&str>,
    scale: u16,
) -> Result<IconKey, String> {
    icons.project_row(document, index, kind, label, point, icon_override, scale)
}

pub fn clear_item(document: &mut impl UiDocumentAccess, index: usize) -> Result<(), String> {
    let item_id = format!("desktop-item-{index}");
    document.visible(&item_id, false)?;
    set_glyph_visibility(document, index, None)?;
    project_item_label(document, index, "", 100)
}

pub fn project_drag_ghost_icon_scaled(
    document: &mut impl UiDocumentAccess,
    icons: &mut ParallelIconService,
    kind: DesktopItemKind,
    icon_override: Option<&str>,
    scale: u16,
) -> Result<(), String> {
    let key = ParallelIconService::icon_key_scaled(kind, icon_override, scale);
    // Miss: Flame fallback now, HIGH request queued, drain replaces it.
    let cached = icons.cached(&key);
    let miss = cached.is_none();
    let mut raster = cached.unwrap_or_else(flame_fallback_raster);
    // Runtime paint currently applies opacity to each primitive, not as a
    // composited subtree. Parent CSS opacity alone never fades an Image node.
    // Fade the RGBA pixels of this drag-only copy, never the icon cache.
    fade_ghost_pixels(&mut raster.pixels);
    document.image_rgba8("desktop-drag-ghost-icon", runtime_image(raster))?;
    document.visible("desktop-drag-ghost-icon", true)?;
    if miss {
        icons.request_high(key);
    }
    Ok(())
}

pub fn project_conversion_preview_icon_scaled(
    document: &mut impl UiDocumentAccess,
    icons: &mut ParallelIconService,
    output: &std::path::Path,
    scale: u16,
) -> Result<(), String> {
    let key = ParallelIconService::icon_key_scaled(
        DesktopItemKind::File,
        Some(file_icon_name(output)),
        scale,
    );
    let cached = icons.cached(&key);
    let miss = cached.is_none();
    document.image_rgba8(
        "desktop-convert-preview-icon",
        runtime_image(cached.unwrap_or_else(flame_fallback_raster)),
    )?;
    if miss {
        icons.request_high(key);
    }
    Ok(())
}

/// Projection-only caret: appends a thin `|` glyph to the edit buffer for
/// display/sync output while a sticky edit is active. A glyph is used because
/// `UiDocumentAccess` has no text-position query, so a separate element cannot
/// be placed at the insertion point of wrapped text. Never persisted to the
/// model; callers pass the raw buffer and render the return value.
#[must_use]
pub fn caret_text(buffer: &str, editing: bool) -> String {
    if editing {
        format!("{buffer}|")
    } else {
        buffer.to_owned()
    }
}

fn project_item_label(
    document: &mut impl UiDocumentAccess,
    index: usize,
    label: &str,
    scale: u16,
) -> Result<(), String> {
    let factor = f32::from(scale) / 100.0;
    let font_size = LABEL_FONT_SIZE_PX * factor;
    let [first, second] = desktop_label_lines(label, LABEL_WIDTH_PX * factor, font_size);
    // The 76px line stays full width; its flex `justify-content:center` centers
    // the text node, which is measured with the renderer's own 0.49em metric.
    // Do not shrink the line to that estimate: real glyphs (notably W) are
    // wider, and a tight box clips the last character.
    document.text(&format!("desktop-label-{index}-line-1"), first)?;
    document.text(&format!("desktop-label-{index}-line-2"), second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_integrations_linux::icons::{IconLookupPurpose, IconOrigin};
    use flamewm_ui_x11::RuntimeImage;
    use std::path::Path;

    // No-op `UiDocumentAccess` methods shared by every probe below; each probe
    // lists only the methods it records or overrides.
    macro_rules! noop_document_common {
        () => {
            fn text(&mut self, _: &str, _: String) -> Result<(), String> {
                Ok(())
            }
            fn image_rgb8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
                Ok(())
            }
            fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
                Ok(())
            }
            fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
                Ok(())
            }
            fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
                Ok(())
            }
            fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
                Ok(())
            }
            fn background_clear(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn border_clear(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
                Ok(())
            }
            fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn overflow(
                &mut self,
                _: &str,
                _: flamewm_ui_x11::Overflow,
                _: flamewm_ui_x11::Overflow,
            ) -> Result<(), String> {
                Ok(())
            }
            fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
                Ok(())
            }
            fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
                Ok(())
            }
        };
    }

    #[test]
    fn projection_caret_is_display_only_and_model_stays_raw() {
        assert_eq!(caret_text("abc", true), "abc|");
        assert_eq!(caret_text("abc", false), "abc");
        assert_eq!(caret_text("", true), "|");
    }

    #[test]
    fn sticky_text_never_draws_horizontal_scrollbar() {
        let css = include_str!("../../../ui/desktop/desktop.css");
        let rule = css
            .lines()
            .find(|line| line.starts_with(".sticky-text {"))
            .expect(".sticky-text rule");
        assert!(rule.contains("overflow-x:hidden"), "{rule}");
        assert!(
            rule.contains("overflow:auto"),
            "vertical scroll kept: {rule}"
        );
    }

    #[test]
    fn sticky_resize_handles_centre_on_outer_corner() {
        // Handles are positioned against the padding box, which sits 1px inside
        // the note's border. A 10px handle centred on the outer corner therefore
        // needs a -6px offset on both edges it anchors to (1 + 5 = 6).
        let css = include_str!("../../../ui/desktop/desktop.css");
        for (corner, horizontal, vertical) in [
            ("nw", "left", "top"),
            ("ne", "right", "top"),
            ("sw", "left", "bottom"),
            ("se", "right", "bottom"),
        ] {
            let selector = format!("sticky-resize-{corner}");
            assert_eq!(css_px(css, &selector, "width"), 10, "{selector} width");
            assert_eq!(css_px(css, &selector, "height"), 10, "{selector} height");
            assert_eq!(
                css_px(css, &selector, horizontal),
                -6,
                "{selector} {horizontal}"
            );
            assert_eq!(
                css_px(css, &selector, vertical),
                -6,
                "{selector} {vertical}"
            );
        }
    }

    #[test]
    fn arbitrary_directory_keeps_generic_folder_semantic() {
        assert_eq!(icon_semantic(DesktopItemKind::Directory), "directory");
        assert_ne!(icon_semantic(DesktopItemKind::Directory), "home");
    }

    /// Resolver over the workspace packaged assets with no user icon data.
    fn workspace_resolver() -> (PathBuf, IconResolver) {
        let packaged_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let missing_data = std::env::temp_dir().join("flamewm-projection-no-such-data-dir");
        let resolver = IconResolver::new(
            packaged_root.clone(),
            vec!["hicolor".to_owned()],
            vec![missing_data],
            0,
        );
        (packaged_root, resolver)
    }

    #[test]
    fn file_item_semantic_icon_is_packaged_document_not_brand_fallback() {
        let (packaged_root, mut resolver) = workspace_resolver();
        let name = icon_name(DesktopItemKind::File);
        let raster = resolver
            .prepare_semantic(name, IconSize::new(ICON_TILE_PX, ICON_TILE_PX))
            .expect("file icon must resolve");
        assert_ne!(
            raster.origin,
            IconOrigin::PackagedFallback,
            "file icon must not fall through to the brand fallback"
        );
        assert_ne!(raster.origin, IconOrigin::PackagedFlame);
        assert_eq!(raster.origin, IconOrigin::PackagedBreeze);
        let expected = packaged_root
            .join("assets/web/breeze")
            .join(format!("{name}.svg"));
        assert!(
            expected.is_file(),
            "packaged document icon missing on disk: {}",
            expected.display()
        );
        assert_eq!(Path::new(&raster.source), expected.as_path());
    }

    #[test]
    fn directory_item_icon_resolves_to_packaged_folder_not_document_edit() {
        let (packaged_root, mut resolver) = workspace_resolver();
        let name = icon_name(DesktopItemKind::Directory);
        assert_eq!(name, "folder");
        let raster = resolver
            .prepare_semantic(name, IconSize::new(ICON_TILE_PX, ICON_TILE_PX))
            .expect("directory icon must resolve");
        assert_eq!(
            Path::new(&raster.source),
            packaged_root.join("assets/web/breeze/folder.svg").as_path(),
            "directory must render folder.svg, not the file pencil"
        );
        assert_ne!(
            ParallelIconService::icon_key(DesktopItemKind::Directory, None),
            ParallelIconService::icon_key(DesktopItemKind::File, None),
            "directory and file slots must not share an icon cache key"
        );
    }

    #[test]
    fn tile_geometry_stays_centered_in_generated_tile() {
        assert_eq!(DESKTOP_TILE_WIDTH, 82.0);
        assert_eq!(DESKTOP_TILE_HEIGHT, 82.0);
    }

    /// Reads an integer `prop:Npx` declaration from the body of `.<selector> { ... }`.
    fn css_px(css: &str, selector: &str, prop: &str) -> i32 {
        let head = format!(".{selector} {{");
        let start = css
            .find(&head)
            .unwrap_or_else(|| panic!("missing rule {head}"))
            + head.len();
        let body = &css[start..start + css[start..].find('}').expect("unclosed rule")];
        let key = format!("{prop}:");
        let value_start = body
            .find(&key)
            .unwrap_or_else(|| panic!("missing {prop} in {selector}"))
            + key.len();
        let value: String = body[value_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        value.parse().expect("integer px value")
    }

    #[test]
    fn desktop_label_is_horizontally_centered_in_item_padding_box() {
        // The item is 82px with a 1px border, so its padding box is 80px wide.
        // A centered label satisfies left * 2 + width == padding box width.
        let css = include_str!("../../../ui/desktop/desktop.css");
        let padding_box = DESKTOP_TILE_WIDTH as i32 - 2;
        let left = css_px(css, "desktop-label", "left");
        let width = css_px(css, "desktop-label", "width");
        assert_eq!(
            left * 2 + width,
            padding_box,
            "label box (left={left}, width={width}) is off-center in {padding_box}px item"
        );
    }

    #[test]
    fn desktop_label_line_centers_text_within_its_box() {
        // The label line is overflow:hidden and sized to the label box; centering
        // keeps the text inside that box when the estimate fits (see text_layout).
        let css = include_str!("../../../ui/desktop/desktop.css");
        let head = ".desktop-label-line {";
        let start = css.find(head).expect("missing .desktop-label-line") + head.len();
        let body = &css[start..start + css[start..].find('}').expect("unclosed rule")];
        assert!(
            body.contains("justify-content:center"),
            "label line must center text in its box, got: {body}"
        );
    }

    #[test]
    fn launcher_metadata_prefers_icon_value_then_kind_default() {
        assert_eq!(
            launcher_icon_name(DesktopItemKind::File, Some("  ")),
            "document-edit"
        );
        assert_eq!(
            launcher_icon_name(DesktopItemKind::File, Some("editor")),
            "editor"
        );
    }

    struct LayerProbe {
        layers: Vec<(String, i32)>,
    }

    impl UiDocumentAccess for LayerProbe {
        noop_document_common!();
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, id: &str, layer: UiLayer) -> Result<(), String> {
            self.layers.push((id.to_owned(), layer.z_index()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MenuShapeProbe {
        corners: Vec<(String, f32)>,
        fills: Vec<(String, UiColor)>,
        backdrops: Vec<(String, Option<u8>)>,
    }

    impl UiDocumentAccess for MenuShapeProbe {
        noop_document_common!();
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.fills.push((id.to_owned(), color));
            Ok(())
        }
        fn corner_radius(&mut self, id: &str, radius_px: f32) -> Result<(), String> {
            self.corners.push((id.to_owned(), radius_px));
            Ok(())
        }
        fn node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
            self.backdrops.push((id.to_owned(), blur_percent));
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            Ok(())
        }
    }

    fn menu_snapshot(decoration: Option<&str>, opacity: Option<u8>) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        if let Some(value) = decoration {
            snapshot
                .values
                .insert("decoration".into(), SettingValue::Text(value.into()));
        }
        if let Some(percent) = opacity {
            snapshot
                .values
                .insert("surfaceOpacity".into(), SettingValue::Percent(percent));
        }
        snapshot
    }

    #[test]
    fn every_desktop_menu_root_takes_decoration_radius_and_surface_fill() {
        let snapshot = menu_snapshot(Some("squared"), Some(50));
        let mut probe = MenuShapeProbe::default();
        for id in MENU_SURFACE_IDS {
            project_menu_surface(&mut probe, id, &snapshot).expect("project menu");
        }
        assert_eq!(probe.corners.len(), MENU_SURFACE_IDS.len());
        for (id, radius) in &probe.corners {
            let expected = if id == "desktop-convert-wheel" {
                164.0
            } else {
                0.0
            };
            assert_eq!(
                *radius, expected,
                "only the radial wheel keeps its circular shape: {id}"
            );
        }
        for (id, color) in &probe.fills {
            // 50% of 255 is 127.5; the surface alpha rounds half up (see surface_fill).
            assert_eq!(color.a, 128, "surfaceOpacity 50% must reach {id}");
        }
    }

    #[test]
    fn every_desktop_menu_root_gets_node_backdrop_from_surface_blur() {
        let mut snapshot = menu_snapshot(None, None);
        snapshot
            .values
            .insert("surfaceBlurPercent".into(), SettingValue::Percent(40));
        let mut probe = MenuShapeProbe::default();
        for id in MENU_SURFACE_IDS {
            project_menu_surface(&mut probe, id, &snapshot).expect("project menu");
        }
        assert_eq!(probe.backdrops.len(), MENU_SURFACE_IDS.len());
        for (id, blur) in &probe.backdrops {
            assert_eq!(*blur, Some(40), "surfaceBlurPercent must reach {id}");
        }
    }

    #[test]
    fn zero_surface_blur_and_hidden_menu_clear_node_backdrop() {
        let mut snapshot = menu_snapshot(None, None);
        snapshot
            .values
            .insert("surfaceBlurPercent".into(), SettingValue::Percent(0));
        let mut probe = MenuShapeProbe::default();
        project_menu_surface(&mut probe, "desktop-menu", &snapshot).expect("project menu");
        clear_menu_backdrop(&mut probe, "sticky-menu").expect("clear");
        assert_eq!(
            probe.backdrops,
            vec![
                ("desktop-menu".to_owned(), Some(0)),
                ("sticky-menu".to_owned(), None),
            ]
        );
    }

    #[test]
    fn desktop_menu_radius_defaults_to_rounded_twelve() {
        for snapshot in [
            menu_snapshot(None, None),
            menu_snapshot(Some("rounded"), None),
        ] {
            let mut probe = MenuShapeProbe::default();
            project_menu_surface(&mut probe, "desktop-menu", &snapshot).expect("project menu");
            assert_eq!(probe.corners, vec![("desktop-menu".to_owned(), 12.0)]);
        }
    }

    fn layer_of(probe: &LayerProbe, id: &str) -> i32 {
        probe
            .layers
            .iter()
            .find(|(entry, _)| entry == id)
            .map(|(_, z)| *z)
            .expect("layer assigned")
    }

    #[test]
    fn document_layers_order_wallpaper_below_item_below_menu_below_modal() {
        let mut probe = LayerProbe { layers: Vec::new() };
        assign_document_layers(&mut probe).expect("assign layers");
        let root = layer_of(&probe, "desktop");
        let wallpaper = layer_of(&probe, "desktop-wallpaper-selection");
        let watermark = layer_of(&probe, "desktop-watermark");
        let item = layer_of(&probe, "desktop-item-0");
        let selection = layer_of(&probe, "desktop-selection");
        let sticky = layer_of(&probe, "sticky-1");
        let menu = layer_of(&probe, "desktop-menu");
        let entry_menu = layer_of(&probe, "desktop-entry-menu");
        let sticky_menu = layer_of(&probe, "sticky-menu");
        let modal = layer_of(&probe, "desktop-confirm");
        let rename = layer_of(&probe, "desktop-rename");
        assert_eq!(root, UiLayer::SurfaceBackground.z_index());
        assert_eq!(wallpaper, UiLayer::Background.z_index());
        assert_eq!(watermark, UiLayer::Content.z_index());
        assert!(root < wallpaper);
        assert!(wallpaper < watermark);
        assert!(watermark <= item);
        assert!(wallpaper < item);
        assert!(item < selection);
        assert!(selection < menu);
        assert!(menu < modal);
        assert_eq!(sticky, UiLayer::Floating.z_index());
        assert_eq!(entry_menu, menu);
        assert_eq!(sticky_menu, menu);
        // Menu hit-test wins: popover layer above content/selection.
        assert!(menu > item && menu > selection);
        // The inline rename editor paints above the items it edits, like the menus.
        assert!(rename > item && rename > selection);
        assert_eq!(rename, menu);
    }

    #[derive(Default)]
    struct VisibilityProbe {
        hidden: Vec<String>,
        shown: Vec<String>,
    }

    #[test]
    fn default_and_custom_image_presentations_project_independent_of_setting_text() {
        let image = WallpaperPresentation::Image(WallpaperRaster {
            source: "packaged/default.webp".to_owned(),
            width: 1,
            height: 1,
            pixels: Arc::new(vec![0, 0, 0, 255]),
        });
        let mut probe = VisibilityProbe::default();
        project_background(&mut probe, &image).expect("project image");
        assert!(
            probe
                .shown
                .contains(&"desktop-wallpaper-selection".to_owned())
        );
        assert!(probe.shown.contains(&"desktop-watermark".to_owned()));
        assert!(!probe.hidden.contains(&"desktop-watermark".to_owned()));

        let mut reset = VisibilityProbe::default();
        project_background(&mut reset, &WallpaperPresentation::DefaultBlackWatermark)
            .expect("project reset fallback");
        assert!(
            reset
                .hidden
                .contains(&"desktop-wallpaper-selection".to_owned())
        );
        assert!(reset.shown.contains(&"desktop-watermark".to_owned()));
    }

    impl UiDocumentAccess for VisibilityProbe {
        noop_document_common!();
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            if visible {
                self.shown.push(id.to_owned());
            } else {
                self.hidden.push(id.to_owned());
            }
            Ok(())
        }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct WallpaperImageProbe {
        sent: Vec<Vec<u8>>,
        shared: Vec<Arc<Vec<u8>>>,
        fail_next: bool,
    }

    impl UiDocumentAccess for WallpaperImageProbe {
        noop_document_common!();
        fn image_rgba8(&mut self, _: &str, image: RuntimeImage) -> Result<(), String> {
            if std::mem::take(&mut self.fail_next) {
                return Err("rejected".to_owned());
            }
            self.sent.push(image.pixels);
            Ok(())
        }
        fn image_rgba8_shared(
            &mut self,
            _: &str,
            _: String,
            _: u32,
            _: u32,
            pixels: Arc<Vec<u8>>,
        ) -> Result<(), String> {
            if std::mem::take(&mut self.fail_next) {
                return Err("rejected".to_owned());
            }
            self.sent.push(pixels.as_ref().clone());
            self.shared.push(pixels);
            Ok(())
        }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            Ok(())
        }
    }

    fn wallpaper_raster(fill: u8) -> WallpaperRaster {
        WallpaperRaster {
            source: "custom.webp".to_owned(),
            width: 1,
            height: 1,
            pixels: Arc::new(vec![fill, fill, fill, 255]),
        }
    }

    #[test]
    fn unchanged_wallpaper_skips_clone_and_changed_wallpaper_resends() {
        let first = wallpaper_raster(10);
        let same = wallpaper_raster(10);
        let changed = wallpaper_raster(200);
        let mut cache = None;
        let mut probe = WallpaperImageProbe::default();

        project_wallpaper_image(&mut probe, &mut cache, &first).expect("first projection");
        assert_eq!(probe.sent, vec![vec![10, 10, 10, 255]]);
        assert!(
            Arc::ptr_eq(&probe.shared[0], &first.pixels),
            "projection must hand over the loader buffer, not a copy"
        );

        // Same loader raster: no image write, document keeps the prior pixels.
        project_wallpaper_image(&mut probe, &mut cache, &first).expect("unchanged projection");
        assert_eq!(probe.sent.len(), 1, "unchanged wallpaper must not resend");

        // Equal content in a new buffer is a new raster and resends.
        project_wallpaper_image(&mut probe, &mut cache, &same).expect("reloaded projection");
        assert_eq!(probe.sent.len(), 2, "replaced raster must resend");

        project_wallpaper_image(&mut probe, &mut cache, &changed).expect("changed projection");
        assert_eq!(probe.sent.last(), Some(&vec![200, 200, 200, 255]));
        assert_eq!(probe.sent.len(), 3);
    }

    #[test]
    fn rejected_wallpaper_write_is_retried_on_next_projection() {
        let raster = wallpaper_raster(42);
        let mut cache = None;
        let mut probe = WallpaperImageProbe {
            fail_next: true,
            ..WallpaperImageProbe::default()
        };
        assert!(project_wallpaper_image(&mut probe, &mut cache, &raster).is_err());
        project_wallpaper_image(&mut probe, &mut cache, &raster).expect("retry");
        assert_eq!(probe.sent.len(), 1);
    }

    #[test]
    fn transient_visibility_hides_all_transient_nodes_before_first_paint() {
        let mut probe = VisibilityProbe::default();
        initialize_transient_visibility(&mut probe).expect("hide transients");
        for id in TRANSIENT_HIDDEN_IDS {
            assert!(probe.hidden.contains(&id.to_owned()), "hides {id}");
        }
        assert!(probe.shown.is_empty());
        // Showing one restores its authored display; unrelated nodes untouched.
        probe
            .visible("desktop-menu", true)
            .expect("show restores authored display");
        assert_eq!(probe.shown, vec!["desktop-menu".to_owned()]);
    }

    #[test]
    fn compiled_desktop_document_has_only_canonical_sticky_slots() {
        let mut document = flamewm_ui_x11::decode_document(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/flamewm-desktop.rwr"
        )))
        .expect("decode compiled desktop document");

        assert!(
            document.visible("sticky-note", false).is_err(),
            "compiled desktop document must not contain the legacy sticky-note node"
        );

        for slot in 1..=STICKY_SLOT_COUNT {
            let note_id = format!("sticky-{slot}");
            assert!(
                document.visible(&note_id, false).is_ok(),
                "compiled desktop document must contain canonical sticky note {slot}"
            );
            let text_id = format!("sticky-{slot}-text");
            assert!(
                document.text(&text_id, String::new()).is_ok(),
                "compiled desktop document must contain canonical sticky text target {slot}"
            );
        }
        assert!(
            document
                .visible(&format!("sticky-{}", STICKY_SLOT_COUNT + 1), false)
                .is_err(),
            "compiled desktop document must not contain a slot beyond the canonical count"
        );
    }

    #[test]
    fn drag_threshold_gates_selection_overlay_show_hide() {
        use flamewm_desktop_core::layout::threshold_passed;
        use flamewm_desktop_core::selection::rubber_visible;
        // Below 5px Euclidean threshold: overlay stays hidden.
        assert!(!threshold_passed(3.0, 3.9));
        assert!(!rubber_visible(3.0, 3.9));
        // At/above threshold: overlay shows.
        assert!(threshold_passed(3.0, 4.0));
        assert!(rubber_visible(3.0, 4.0));
        assert!(rubber_visible(-6.0, 0.0));
    }

    #[test]
    fn entry_menu_is_transient_and_topmost_over_items() {
        // Menu surfaces start hidden before first paint (no flash).
        let mut probe = VisibilityProbe::default();
        initialize_transient_visibility(&mut probe).expect("hide transients");
        for id in ["desktop-menu", "desktop-entry-menu", "sticky-menu"] {
            assert!(probe.hidden.contains(&id.to_owned()), "hides {id}");
        }
        // Popover/menu layer sits above entry content and selection so the
        // topmost hit-test lands on the open menu, not the item beneath.
        let mut layers = LayerProbe { layers: Vec::new() };
        assign_document_layers(&mut layers).expect("assign layers");
        let item = layer_of(&layers, "desktop-item-0");
        let selection = layer_of(&layers, "desktop-selection");
        let menu = layer_of(&layers, "desktop-menu");
        let entry_menu = layer_of(&layers, "desktop-entry-menu");
        assert_eq!(entry_menu, menu);
        assert!(menu > item && menu > selection);
    }

    #[test]
    fn icon_purpose_routes_launcher_override_to_application() {
        assert_eq!(
            icon_purpose(DesktopItemKind::DesktopLauncher, true),
            IconLookupPurpose::Application
        );
        assert_eq!(
            icon_purpose(DesktopItemKind::DesktopLauncher, false),
            IconLookupPurpose::Semantic
        );
        assert_eq!(
            icon_purpose(DesktopItemKind::Directory, true),
            IconLookupPurpose::Semantic
        );
        assert_eq!(
            icon_purpose(DesktopItemKind::File, false),
            IconLookupPurpose::Semantic
        );
        assert_eq!(
            icon_purpose(DesktopItemKind::TrashPseudo, false),
            IconLookupPurpose::Semantic
        );
    }

    /// Polls `apply_icon_theme` until the requested chain is installed.
    /// THEME_GENERATION is process-global and each service numbers its own generations, so
    /// parallel theme tests race on it. Tests that read or bump it take this lock.
    static THEME_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn await_icon_theme(icons: &mut ParallelIconService, theme: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !icons.apply_icon_theme(theme) {
            assert!(
                std::time::Instant::now() < deadline,
                "requested icon theme must install within deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn apply_icon_theme_installs_requested_chain_and_bumps_generation() {
        let _theme_guard = THEME_STATE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut icons = ParallelIconService::spawn(
            std::env::temp_dir().join("flamewm-projection-apply-theme-no-such-root"),
        );
        let before = THEME_GENERATION.load(Ordering::Acquire);
        await_icon_theme(&mut icons, "flame-apply-no-such-theme");
        assert!(THEME_GENERATION.load(Ordering::Acquire) > before);
        assert_eq!(
            icons.service.shared_index().theme_names,
            flamewm_integrations_linux::icons::theme_chain(Some("flame-apply-no-such-theme"))
        );
    }

    #[test]
    fn apply_icon_theme_stays_satisfied_after_watch_refresh() {
        let _theme_guard = THEME_STATE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // A watch refresh rebuilds from the installed chain, so the pending icon theme request
        // must still read as satisfied; otherwise icon_theme_pending would stay true and retry
        // request_theme_change on every tick.
        let mut icons = ParallelIconService::spawn(
            std::env::temp_dir().join("flamewm-projection-watch-after-apply-no-such-root"),
        );
        await_icon_theme(&mut icons, "flame-watch-after-apply-no-such-theme");
        for _ in 0..3 {
            icons.refresh_theme();
            assert!(icons.apply_icon_theme("flame-watch-after-apply-no-such-theme"));
        }
    }

    #[test]
    fn refresh_theme_unchanged_index_returns_false_and_keeps_mirror() {
        let _theme_guard = THEME_STATE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut icons = ParallelIconService::spawn(
            std::env::temp_dir().join("flamewm-projection-refresh-theme-no-such-root"),
        );
        let before = THEME_GENERATION.load(Ordering::Acquire);
        assert!(!icons.refresh_theme());
        assert_eq!(THEME_GENERATION.load(Ordering::Acquire), before);
    }

    #[test]
    fn images_documents_archives_use_distinct_icons() {
        assert_eq!(file_icon_name(Path::new("tree.PNG")), "image-png");
        assert_eq!(file_icon_name(Path::new("tree.webp")), "image-webp");
        assert_eq!(file_icon_name(Path::new("report.pdf")), "application-pdf");
        assert_eq!(file_icon_name(Path::new("notes.md")), "text-markdown");
        assert_eq!(file_icon_name(Path::new("data.zip")), "application-zip");
        assert_eq!(file_icon_name(Path::new("untitled")), "text-x-generic");
    }

    #[test]
    fn ghost_alpha_fades_actual_pixels_without_colorkey() {
        let mut pixels = vec![17, 29, 47, 255, 101, 67, 23, 128, 1, 2, 3, 0];
        fade_ghost_pixels(&mut pixels);
        assert_eq!(pixels, vec![17, 29, 47, 128, 101, 67, 23, 64, 1, 2, 3, 0]);
    }

    #[test]
    fn global_font_defaults_read_typed_live_values_and_bound_size_offset() {
        let mut settings = SettingsSnapshot::new(1);
        settings
            .values
            .insert("fontSizeOffset".into(), SettingValue::Integer(3));
        settings
            .values
            .insert("fontBold".into(), SettingValue::Boolean(true));
        assert_eq!(global_font_defaults(&settings), (3.0, true));

        settings
            .values
            .insert("fontSizeOffset".into(), SettingValue::Integer(99));
        assert_eq!(global_font_defaults(&settings), (4.0, true));
        settings
            .values
            .insert("fontBold".into(), SettingValue::Text("true".into()));
        assert_eq!(global_font_defaults(&settings), (4.0, false));
    }
}
