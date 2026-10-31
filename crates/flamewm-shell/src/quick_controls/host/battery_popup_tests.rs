use super::*;
use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
use flamewm_ui_x11::test_support::RecordingUiDoc;

fn power(cpu_percent: Option<u8>) -> PowerSnapshot {
    PowerSnapshot {
        available: ServiceAvailability::Available,
        present: true,
        percent: 80,
        state: PowerState::Discharging,
        cpu_percent,
        power_rate_mw: None,
    }
}

fn project_battery(doc: &mut RecordingUiDoc, cpu_percent: Option<u8>) {
    crate::projection::project_battery_menu(doc, &power(cpu_percent), 268.0)
        .expect("battery menu projects");
}

fn text_of<'a>(doc: &'a RecordingUiDoc, id: &str) -> Option<&'a str> {
    doc.texts.get(id).map(String::as_str)
}

#[test]
fn battery_kind_wire_name_and_popup_root_are_battery() {
    assert_eq!(QuickControlKind::Battery.as_str(), "battery");
    assert_eq!(
        popup::role_for_kind(QuickControlKind::Battery).intrinsic_node_id(),
        "battery-popup"
    );
}

#[test]
fn battery_menu_projects_cpu_text_and_refreshes_when_cpu_changes() {
    let mut doc = RecordingUiDoc::default();
    project_battery(&mut doc, Some(37));
    assert_eq!(text_of(&doc, "battery-cpu"), Some("CPU usage: 37%"));
    assert_eq!(text_of(&doc, "battery-percent"), Some("80%"));
    // A later snapshot with a new CPU sample must change the projected text.
    project_battery(&mut doc, Some(64));
    assert_eq!(text_of(&doc, "battery-cpu"), Some("CPU usage: 64%"));
    project_battery(&mut doc, None);
    assert_eq!(text_of(&doc, "battery-cpu"), Some("CPU usage: --%"));
}

#[test]
fn battery_artifact_decodes_and_projects_on_the_real_document() {
    let mut document = decode_document(BATTERY_ARTIFACT).expect("battery artifact must decode");
    document
        .visible("battery-popup", true)
        .expect("battery root must resolve");
    crate::projection::project_battery_menu(&mut document, &power(Some(12)), 268.0)
        .expect("battery menu projects on the compiled document");
}
