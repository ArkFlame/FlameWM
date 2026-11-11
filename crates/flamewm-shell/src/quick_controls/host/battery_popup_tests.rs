use super::*;
use flamewm_api::system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioSnapshot, PowerSnapshot, PowerState,
    ServiceAvailability,
};
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

fn backlight(
    availability: ServiceAvailability,
    percent: u8,
) -> flamewm_api::system::BrightnessSnapshot {
    flamewm_api::system::BrightnessSnapshot {
        availability,
        generation: 7,
        percent,
    }
}

#[test]
fn brightness_row_shows_percent_and_dashes_when_absent() {
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_brightness_row(
        &mut doc,
        &backlight(ServiceAvailability::Available, 40),
    )
    .expect("row projects");
    assert_eq!(text_of(&doc, "battery-brightness-value"), Some("40%"));
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_brightness_row(
        &mut doc,
        &backlight(ServiceAvailability::Unavailable, 40),
    )
    .expect("row projects");
    assert_eq!(text_of(&doc, "battery-brightness-value"), Some("--"));
}

#[test]
fn brightness_step_clamps_and_carries_generation() {
    use flamewm_api::system::{BrightnessAction, SystemAction};
    let step = |percent, up| {
        crate::projection::brightness_step_action(
            &backlight(ServiceAvailability::Available, percent),
            up,
        )
    };
    assert_eq!(
        step(40, true),
        Some(SystemAction::SetBrightness(BrightnessAction {
            percent: 50,
            generation: 7,
        }))
    );
    assert_eq!(
        step(95, true),
        Some(SystemAction::SetBrightness(BrightnessAction {
            percent: 100,
            generation: 7,
        }))
    );
    assert_eq!(
        step(4, false),
        Some(SystemAction::SetBrightness(BrightnessAction {
            percent: 0,
            generation: 7,
        }))
    );
    assert!(step(100, true).is_none());
    assert!(step(0, false).is_none());
    assert!(crate::projection::brightness_step_action(
        &backlight(ServiceAvailability::Unavailable, 40),
        true,
    )
    .is_none());
}

#[test]
fn battery_popup_brightness_row_fits_the_popup_and_is_wired() {
    let html = include_str!("../../../../../ui/shell/battery.html");
    assert!(html.contains("data-action=\"brightness.down\""));
    assert!(html.contains("data-action=\"brightness.up\""));
    assert!(html.contains("id=\"battery-brightness-value\""));
    assert!(
        html.contains("height:184px"),
        "popup must grow to hold the microphone row"
    );
    // Row spans top 136..154, inside the 160px popup.
    assert!(html.contains("top:136px; width:260px; height:18px"));
}

#[test]
fn battery_popup_microphone_row_fits_the_popup_and_is_wired() {
    let html = include_str!("../../../../../ui/shell/battery.html");
    for action in ["input.mute", "input.down", "input.up"] {
        assert!(
            html.contains(&format!("data-action=\"{action}\"")),
            "{action}"
        );
    }
    assert!(html.contains("id=\"battery-input-value\""));
    assert!(html.contains("id=\"battery-input-mute\""));
    // Each control names itself for assistive tech.
    assert_eq!(html.matches("aria-label=\"").count(), 3);
    // Row spans top 160..178, inside the 184px popup.
    assert!(html.contains("top:160px; width:260px; height:18px"));
}

fn input_source(volume_percent: u8, muted: bool) -> AudioSnapshot {
    let mut audio = AudioSnapshot::default();
    audio.endpoints.push(AudioEndpointSnapshot {
        id: 7,
        kind: AudioEndpointKind::Source,
        name: "mic".to_owned(),
        description: "Microphone".to_owned(),
        volume_percent,
        muted,
        is_default: true,
    });
    audio
}

#[test]
fn input_row_shows_default_source_level_and_mute_label() {
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_input_row(&mut doc, &input_source(40, false)).expect("row projects");
    assert_eq!(text_of(&doc, "battery-input-value"), Some("40%"));
    assert_eq!(text_of(&doc, "battery-input-mute"), Some("Mute"));
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_input_row(&mut doc, &input_source(40, true)).expect("row projects");
    assert_eq!(text_of(&doc, "battery-input-mute"), Some("Unmute"));
}

#[test]
fn input_row_shows_dashes_without_a_default_source() {
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_input_row(&mut doc, &AudioSnapshot::default())
        .expect("row projects");
    assert_eq!(text_of(&doc, "battery-input-value"), Some("--"));
}

fn profiles(
    active: Option<flamewm_api::system::PowerProfile>,
) -> flamewm_api::system::PowerProfilesSnapshot {
    use flamewm_api::system::PowerProfile::*;
    flamewm_api::system::PowerProfilesSnapshot {
        available: true,
        active,
        profiles: vec![PowerSaver, Balanced, Performance],
    }
}

#[test]
fn power_profile_row_shows_active_label_and_dashes_when_absent() {
    use flamewm_api::system::PowerProfile::Balanced;
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_power_profile_row(&mut doc, &profiles(Some(Balanced)))
        .expect("row projects");
    assert_eq!(text_of(&doc, "battery-profile"), Some("Balanced"));
    let mut doc = RecordingUiDoc::default();
    crate::projection::project_power_profile_row(&mut doc, &Default::default())
        .expect("row projects");
    assert_eq!(text_of(&doc, "battery-profile"), Some("--"));
}

#[test]
fn power_profile_cycle_wraps_through_offered_profiles() {
    use flamewm_api::system::{PowerProfile, SystemAction};
    let next = |active| match crate::projection::power_profile_cycle_action(&profiles(Some(active)))
    {
        Some(SystemAction::SetPowerProfile(profile)) => profile,
        other => panic!("unexpected action {other:?}"),
    };
    assert_eq!(next(PowerProfile::PowerSaver), PowerProfile::Balanced);
    assert_eq!(next(PowerProfile::Performance), PowerProfile::PowerSaver);
    assert!(crate::projection::power_profile_cycle_action(&Default::default()).is_none());
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
