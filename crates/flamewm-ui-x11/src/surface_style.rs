//! Shared renderer-facing mapping for surface materials. Geometry, theme fill and
//! cached projection state stay with their existing owners.
use flamewm_api::settings::SettingsSnapshot;
use flamewm_ui_core::surface::{SurfaceFill, surface_fill, surface_stroke};

use crate::{UiColor, UiDocumentAccess};

impl From<SurfaceFill> for UiColor {
    fn from(value: SurfaceFill) -> Self {
        Self {
            r: (value.rgb >> 16) as u8,
            g: (value.rgb >> 8) as u8,
            b: value.rgb as u8,
            a: value.alpha,
        }
    }
}

/// The same RGBA surface fill for every X11 UI root.
#[must_use]
pub fn surface_background(snapshot: &SettingsSnapshot) -> UiColor {
    surface_fill(snapshot).into()
}

/// The same 1px contour for every X11 UI root, independent of its fill.
#[must_use]
pub fn surface_border(snapshot: &SettingsSnapshot) -> UiColor {
    surface_stroke(snapshot).into()
}

/// All UI roots use this projection; their layouts and background treatments
/// remain owned by the individual components.
pub fn project_surface_border(
    document: &mut impl UiDocumentAccess,
    root: &str,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    document.border(root, surface_border(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::settings::SettingValue;

    #[test]
    fn default_material_and_contour_follow_shared_contract() {
        let snapshot = SettingsSnapshot::new(1);
        assert_eq!(
            surface_background(&snapshot),
            UiColor {
                r: 25,
                g: 27,
                b: 29,
                a: 127
            }
        );
        assert_eq!(
            surface_border(&snapshot),
            UiColor {
                r: 255,
                g: 255,
                b: 255,
                a: 63
            }
        );
    }

    #[test]
    fn border_and_fill_are_independently_configurable() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceColor".into(), SettingValue::Text("#123456".into()));
        snapshot
            .values
            .insert("surfaceOpacity".into(), SettingValue::Percent(0));
        snapshot.values.insert(
            "surfaceBorderColor".into(),
            SettingValue::Text("#FEDCBA".into()),
        );
        snapshot
            .values
            .insert("surfaceBorderOpacity".into(), SettingValue::Percent(100));
        assert_eq!(
            surface_background(&snapshot),
            UiColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 0
            }
        );
        assert_eq!(
            surface_border(&snapshot),
            UiColor {
                r: 0xfe,
                g: 0xdc,
                b: 0xba,
                a: 255
            }
        );
    }
}
