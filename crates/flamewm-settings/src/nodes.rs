//! Split from `lib.rs` (`nodes`); see crate docs.

use super::*;

/// Deletes the managed copy behind a cleared Start icon. Empty, unmanaged and
/// already-missing paths are left alone; the store never deletes foreign files.
pub(crate) fn remove_managed_media(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    let Some(root) = MediaStore::default_root() else {
        return Ok(());
    };
    let path = Path::new(path);
    if !MediaStore::new(root).is_managed(path) {
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove start icon copy: {error}"))
        }
        _ => Ok(()),
    }
}

/// Authoritative accent value behind each palette swatch in the Appearance page.
pub(crate) fn accent_for_swatch(name: &str) -> Option<&'static str> {
    match name {
        "red" => Some("#EF4048"),
        "blue" => Some("#3DAEE9"),
        "purple" => Some("#9B59B6"),
        "teal" => Some("#1ABC9C"),
        _ => None,
    }
}

/// An opaque swatch colour for a `#RRGGBB` value.
pub(crate) fn solid_color(value: &str) -> Result<UiColor, String> {
    Ok(UiColor {
        a: 255,
        ..accent_color(value)?
    })
}

/// Generates `UiDocumentAccess` methods that forward one write to `inner` and record its failure.
macro_rules! forward_writes {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) -> Result<(), String> {
                if let Err(error) = self.inner.$name($($arg),*) {
                    self.failures.push(error);
                }
                Ok(())
            }
        )*
    };
}

/// Forwards every write to `inner`, but a rejected write (for example a missing node id)
/// is recorded instead of aborting the sync, so one bad node cannot blank the whole page.
/// `into_result` reports all recorded failures together.
pub(crate) struct NodeFailures<'a, D: UiDocumentAccess + ?Sized> {
    pub(crate) inner: &'a mut D,
    pub(crate) failures: Vec<String>,
}

impl<D: UiDocumentAccess + ?Sized> NodeFailures<'_, D> {
    pub(crate) fn into_result(self) -> Result<(), String> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(self.failures.join("; "))
        }
    }
}

impl<D: UiDocumentAccess + ?Sized> UiDocumentAccess for NodeFailures<'_, D> {
    forward_writes! {
        text(id: &str, text: String);
        image_rgb8(id: &str, image: flamewm_ui_x11::RuntimeImage);
        image_rgba8(id: &str, image: flamewm_ui_x11::RuntimeImage);
        image_reset(id: &str);
        visible(id: &str, visible: bool);
        toggle(track_id: &str, knob_id: &str, checked: bool);
        position(id: &str, left: f32, top: f32);
        size(id: &str, width: f32, height: f32);
        background(id: &str, color: UiColor);
        border(id: &str, color: UiColor);
        corner_radius(id: &str, radius_px: f32);
        node_backdrop(id: &str, blur_percent: Option<u8>);
        foreground(id: &str, color: UiColor);
        background_clear(id: &str);
        border_clear(id: &str);
        foreground_clear(id: &str);
        font_size(id: &str, size_px: f32);
        font_weight(id: &str, weight: u16);
        ui_font_defaults(size_offset: f32, bold: bool);
        font_family(family: String);
        color_variable(name: &str, color: UiColor);
        overflow(id: &str, x: flamewm_ui_x11::Overflow, y: flamewm_ui_x11::Overflow);
        layer(id: &str, layer: flamewm_ui_core::style::UiLayer);
    }
}

/// Paints every accent-coloured node (selected-sidebar line and ON toggles) with the live accent.
pub(crate) fn sync_accent_nodes(
    document: &mut impl UiDocumentAccess,
    accent: &str,
) -> Result<(), String> {
    let color = solid_color(accent)?;
    for id in ACCENT_NODE_IDS {
        document.background(id, color)?;
    }
    document.color_variable("--accent", color)?;
    document.color_variable("--accent-soft", UiColor { a: 0x38, ..color })
}

/// Window body glass (GlassRegular, the menu surface material), backdrop blur beneath it, and the
/// grouped-card overlay. The sidebar and content areas stay transparent and show this body.
/// Every value comes from `glass::resolve`, so Reduce transparency makes the body opaque with no blur.
pub(crate) fn project_glass_roles(
    document: &mut impl UiDocumentAccess,
    snapshot: &flamewm_api::settings::SettingsSnapshot,
) -> Result<(), String> {
    use flamewm_ui_core::glass::{MaterialRole, RendererCaps, ResolvedMaterial};
    let appearance = flamewm_ui_core::glass::appearance_from_settings(snapshot);
    let caps = RendererCaps {
        backdrop_blur: true,
    };
    let resolve = |role| flamewm_ui_core::glass::resolve(role, appearance, 100, caps);
    let rgba = |material: ResolvedMaterial| UiColor {
        r: (material.tint_rgb >> 16) as u8,
        g: (material.tint_rgb >> 8) as u8,
        b: material.tint_rgb as u8,
        a: (u32::from(material.alpha_permille.min(1000)) * 255 / 1000) as u8,
    };
    let body = resolve(MaterialRole::GlassRegular);
    document.background("settings-root", rgba(body))?;
    document.node_backdrop(
        "settings-root",
        Some(flamewm_ui_core::glass::blur_percent_from_px(body.blur_px)),
    )?;
    // Group fill: a black overlay at 20% alpha on the shared body glass. Reduce Transparency
    // makes the body opaque, so the same 20% darkening is applied to its solid colour instead.
    document.color_variable(
        "--card",
        card_fill(appearance.reduce_transparency, body.tint_rgb),
    )
}

/// The group-zone fill: a 20% black overlay on glass, or a solid 20%-darker shade of the
/// opaque body when Reduce Transparency is on.
pub(crate) fn card_fill(reduce_transparency: bool, body_rgb: u32) -> UiColor {
    if reduce_transparency {
        let shade = |channel: u32| ((channel * 4) / 5) as u8;
        UiColor {
            r: shade(body_rgb >> 16 & 0xff),
            g: shade(body_rgb >> 8 & 0xff),
            b: shade(body_rgb & 0xff),
            a: 0xff,
        }
    } else {
        UiColor {
            r: 0,
            g: 0,
            b: 0,
            a: 0x33,
        }
    }
}
