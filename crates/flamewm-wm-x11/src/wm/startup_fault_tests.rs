//! Fault-injection coverage for staged X11 startup fallbacks. No X server is
//! needed: each decision is a pure function over the failure inputs, and every
//! degrading stage is checked to produce a failure record.
use super::{
    CORE_FALLBACK_EXTENSIONS, ErrorKind, INPUT_FALLBACK_EXTENSIONS, RandrQuery, StartupFault,
    StartupReport, connection_lost_fault, core_fallback_fault, missing_extension_fault,
    randr_event_fault, second_wm_fault,
};
use crate::{empty_randr_resources, randr_resource_fault};

fn record(fault: Option<StartupFault>) -> StartupFault {
    fault.expect("degrading stage must produce a failure record")
}

#[test]
fn randr_present_with_successful_selection_has_no_fault() {
    assert_eq!(randr_event_fault(RandrQuery::Present, None), None);
}

#[test]
fn randr_absent_degrades_to_root_geometry_with_a_record() {
    let fault = record(randr_event_fault(RandrQuery::Absent, None));
    assert_eq!(fault.stage, "extension");
    assert!(fault.message.contains("RandR absent"));
    assert!(fault.message.contains("root geometry is used"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn randr_query_failure_degrades_with_a_record() {
    let fault = record(randr_event_fault(
        RandrQuery::Failed("connection lost".into()),
        None,
    ));
    assert_eq!(fault.stage, "randr-events");
    assert!(
        fault
            .message
            .contains("RandR query failed: connection lost")
    );
    assert!(fault.message.contains("root geometry is used"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn randr_selection_failure_degrades_with_a_record() {
    let fault = record(randr_event_fault(
        RandrQuery::Present,
        Some("Access denied".into()),
    ));
    assert_eq!(fault.stage, "randr-events");
    assert!(
        fault
            .message
            .contains("RandR event selection failed: Access denied")
    );
    assert!(
        fault
            .message
            .contains("root geometry is used until restart")
    );
    assert_eq!(fault.exit_code, None);
}

#[test]
fn second_wm_on_root_selection_is_fatal_with_a_clear_record() {
    let fault = record(second_wm_fault(Some(ErrorKind::Access)));
    assert_eq!(fault.stage, "root-selection");
    assert_eq!(
        fault.message,
        "flamewm: another window manager is already running on this display"
    );
    assert_eq!(fault.exit_code, Some(1));
}

#[test]
fn unrelated_selection_errors_do_not_report_a_second_wm() {
    assert_eq!(second_wm_fault(Some(ErrorKind::Window)), None);
    assert_eq!(second_wm_fault(None), None);
}

#[test]
fn missing_extension_degrades_with_a_record() {
    let fault = missing_extension_fault("XInputExtension", "raw escape keys disabled");
    assert_eq!(fault.stage, "extension");
    assert!(fault.message.starts_with("flamewm: XInputExtension absent"));
    assert!(fault.message.contains("raw escape keys disabled"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn randr_resources_failure_falls_back_to_root_geometry_with_a_record() {
    let fault = randr_resource_fault("no reply".into());
    assert_eq!(fault.stage, "randr-resources");
    assert!(
        fault
            .message
            .contains("RandR screen resources unavailable: no reply")
    );
    assert!(fault.message.contains("falling back to root geometry"));
    assert_eq!(fault.exit_code, None);

    let (timestamp, outputs, modes) = empty_randr_resources();
    assert_eq!(timestamp, 0);
    assert!(outputs.is_empty());
    assert!(modes.is_empty());
}

#[test]
fn every_core_fallback_extension_is_audited_once() {
    assert_eq!(CORE_FALLBACK_EXTENSIONS.len(), 4);
    let names: Vec<&str> = CORE_FALLBACK_EXTENSIONS
        .iter()
        .map(|(_, name, _)| *name)
        .collect();
    assert_eq!(names, ["RENDER", "COMPOSITE", "DAMAGE", "XFIXES"]);
}

#[test]
fn absent_core_fallback_extension_records_one_fault_present_records_none() {
    for &(bytes, name, detail) in CORE_FALLBACK_EXTENSIONS {
        assert_eq!(std::str::from_utf8(bytes).unwrap(), name);
        assert_eq!(core_fallback_fault(name, detail, true), None);
        let fault = record(core_fallback_fault(name, detail, false));
        assert_eq!(fault.stage, "extension");
        assert!(
            fault
                .message
                .starts_with(&format!("flamewm: {name} absent"))
        );
        assert!(fault.message.contains(detail));
        assert_eq!(fault.exit_code, None);
    }
}

#[test]
fn xinput2_absence_degrades_with_a_record() {
    let fault = record(core_fallback_fault("XInputExtension", "no raw keys", false));
    assert_eq!(fault.stage, "extension");
    assert!(fault.message.starts_with("flamewm: XInputExtension absent"));
    assert!(fault.message.contains("blast radius: no raw keys"));
    assert_eq!(fault.exit_code, None);
    assert_eq!(
        core_fallback_fault("XInputExtension", "no raw keys", true),
        None
    );
}

#[test]
fn shape_absence_degrades_with_a_record() {
    let fault = record(core_fallback_fault("SHAPE", "no input shapes", false));
    assert!(fault.message.starts_with("flamewm: SHAPE absent"));
    assert!(fault.message.contains("blast radius: no input shapes"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn xkb_absence_degrades_with_a_record() {
    let fault = record(core_fallback_fault("XKB", "no layout state", false));
    assert!(fault.message.starts_with("flamewm: XKB absent"));
    assert!(fault.message.contains("blast radius: no layout state"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn input_fallback_extensions_are_audited_once() {
    let names: Vec<&str> = INPUT_FALLBACK_EXTENSIONS
        .iter()
        .map(|(_, name, _)| *name)
        .collect();
    assert_eq!(names, ["XInputExtension", "SHAPE", "XKB"]);
    for &(bytes, name, detail) in INPUT_FALLBACK_EXTENSIONS {
        assert_eq!(std::str::from_utf8(bytes).unwrap(), name);
        assert_eq!(core_fallback_fault(name, detail, true), None);
        assert_eq!(
            record(core_fallback_fault(name, detail, false)).stage,
            "extension"
        );
    }
}

#[test]
fn lost_connection_is_fatal_with_a_blast_radius_record() {
    let fault = connection_lost_fault("Broken pipe");
    assert_eq!(fault.stage, "connection");
    assert!(fault.message.contains("X connection lost: Broken pipe"));
    assert!(fault.message.contains("blast radius"));
    assert_eq!(fault.exit_code, Some(1));
}

#[test]
fn startup_report_renders_one_message_with_every_blast_radius() {
    let mut report = StartupReport::default();
    assert!(report.is_empty());
    report.push(missing_extension_fault("XInputExtension", "no raw keys"));
    report.push(missing_extension_fault("SHAPE", "no input shapes"));
    report.push(connection_lost_fault("reset by peer"));
    let rendered = report.render();
    assert!(rendered.starts_with("flamewm: startup degradation report"));
    assert_eq!(rendered.lines().count(), 4);
    assert_eq!(rendered.matches("blast radius").count(), 3);
}
