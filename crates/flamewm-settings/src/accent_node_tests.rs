use std::collections::BTreeMap;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::ControlResponse;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use super::test_transport::ScriptedTransport;
use super::{ACCENT_NODE_IDS, SettingsApplication, solid_color};

#[derive(Default)]
struct ColorDocument {
    backgrounds: BTreeMap<String, UiColor>,
    reject_positions: bool,
}

impl UiDocumentAccess for ColorDocument {
    fn color_variable(&mut self, _name: &str, _color: UiColor) -> Result<(), String> {
        Ok(())
    }

    fn text(&mut self, _id: &str, _text: String) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn visible(&mut self, _id: &str, _visible: bool) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn toggle(&mut self, _track_id: &str, _knob_id: &str, _checked: bool) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_track_id)?;
        crate::ui_contract::require_markup_id(_knob_id)
    }

    fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)?;
        if self.reject_positions {
            return Err(format!("position rejected for {_id}"));
        }
        Ok(())
    }

    fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(id)?;
        self.backgrounds.insert(id.to_owned(), color);
        Ok(())
    }

    fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn background_clear(&mut self, _id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn border_clear(&mut self, _id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn font_size(&mut self, _id: &str, _size_px: f32) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn font_weight(&mut self, _id: &str, _weight: u16) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn overflow(
        &mut self,
        _id: &str,
        _x: flamewm_ui_x11::Overflow,
        _y: flamewm_ui_x11::Overflow,
    ) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }

    fn layer(&mut self, _id: &str, _layer: flamewm_ui_core::style::UiLayer) -> Result<(), String> {
        crate::ui_contract::require_markup_id(_id)
    }
}

fn application_with_accent(accent: &str) -> SettingsApplication<ScriptedTransport> {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("accent".to_owned(), SettingValue::Text(accent.to_owned()));
    snapshot
        .values
        .insert("animationsEnabled".to_owned(), SettingValue::Boolean(true));
    let mut transport = ScriptedTransport::default();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(snapshot)));
    let mut application = SettingsApplication::new(transport);
    application
        .client_mut()
        .refresh_settings()
        .expect("initial snapshot");
    application
}

#[test]
fn accent_nodes_carry_the_active_accent_not_the_compiled_red() {
    for accent in ["#3DAEE9", "#9B59B6", "#1ABC9C", "#123456"] {
        let application = application_with_accent(accent);
        let mut document = ColorDocument::default();
        application.sync(&mut document).expect("sync");
        let expected = solid_color(accent).expect("accent");
        for id in ACCENT_NODE_IDS {
            assert_eq!(
                document.backgrounds.get(*id),
                Some(&expected),
                "{id} under {accent}"
            );
        }
        assert!(!document.backgrounds.contains_key("anim-global-off"));
    }
}

#[test]
fn sync_reports_recorded_write_failures_alongside_a_later_error() {
    let application = application_with_accent("not-a-colour");
    let mut document = ColorDocument {
        reject_positions: true,
        ..ColorDocument::default()
    };
    let error = application.sync(&mut document).expect_err("sync must fail");
    assert!(
        error.contains("position rejected for nav-selection"),
        "{error}"
    );
    assert!(error.contains("accent is not a #RRGGBB value"), "{error}");
}

#[test]
fn red_swatch_still_paints_red_accent_nodes() {
    let application = application_with_accent("#EF4048");
    let mut document = ColorDocument::default();
    application.sync(&mut document).expect("sync");
    for id in ACCENT_NODE_IDS {
        let color = document.backgrounds.get(*id).expect("accent node painted");
        assert_eq!(
            (color.r, color.g, color.b, color.a),
            (0xEF, 0x40, 0x48, 255),
            "{id}"
        );
    }
}

#[test]
fn accent_registry_covers_every_toggle_on_twin_in_markup() {
    let html = include_str!("../../../ui/settings.html");
    let marker = "toggle toggle-on\x22 id=\x22";
    let ids: Vec<&str> = html
        .split(marker)
        .skip(1)
        .map(|after| &after[..after.find('\x22').expect("closed toggle id")])
        .collect();
    assert!(!ids.is_empty(), "no toggle-on ids found in settings.html");
    let missing: Vec<&str> = ids
        .into_iter()
        .filter(|id| !ACCENT_NODE_IDS.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "toggle-on ids missing from ACCENT_NODE_IDS: {missing:?}"
    );
}
