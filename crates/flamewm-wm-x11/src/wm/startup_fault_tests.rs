//! Fault-injection coverage for staged X11 startup fallbacks. No X server is
//! needed: each decision is a pure function over the failure inputs, and every
//! degrading stage is checked to produce a failure record.
use super::{
    CORE_FALLBACK_EXTENSIONS, ErrorKind, INPUT_FALLBACK_EXTENSIONS, RandrQuery, StartupFault,
    StartupReport, connection_lost_fault, core_fallback_fault, missing_extension_fault,
    randr_event_fault, second_wm_fault,
};
use super::{ExtensionQuery, is_destroyed_window_race, optional_extension_fault, renderer_fault};
use crate::startup_failure::{
    connect_cause, startup_failure_line, startup_failure_report, startup_stage,
};
use crate::{empty_randr_resources, randr_resource_fault};
use x11rb::errors::ReplyOrIdError;
use x11rb::x11_utils::X11Error;

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
        "flamewm: another window manager is already running on this display; blast radius: this session has no WM and exits 1"
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

fn x11_error(kind: ErrorKind) -> ReplyOrIdError {
    ReplyOrIdError::X11Error(X11Error {
        error_kind: kind,
        error_code: 3,
        sequence: 1,
        bad_value: 0,
        minor_opcode: 0,
        major_opcode: 8,
        extension_name: None,
        request_name: Some("MapWindow"),
    })
}

#[test]
fn startup_scan_skips_only_destroyed_window_races() {
    assert!(is_destroyed_window_race(&x11_error(ErrorKind::Window)));
    assert!(is_destroyed_window_race(&x11_error(ErrorKind::Drawable)));
    assert!(is_destroyed_window_race(&x11_error(ErrorKind::Match)));
    assert!(!is_destroyed_window_race(&x11_error(ErrorKind::Access)));
    assert!(!is_destroyed_window_race(&ReplyOrIdError::IdsExhausted));
}

#[test]
fn renderer_open_failure_pushes_a_renderer_record() {
    let fault = renderer_fault("no font");
    assert_eq!(fault.stage, "renderer");
    assert!(
        fault
            .message
            .contains("decoration renderer unavailable: no font")
    );
    assert!(
        fault
            .message
            .contains("decorations and cursors use core frames")
    );
    assert_eq!(fault.exit_code, None);
}

#[test]
fn failed_extension_query_is_unknown_not_absent() {
    let fault = record(optional_extension_fault(
        "SHAPE",
        "no input shapes",
        ExtensionQuery::Unknown("Broken pipe".into()),
    ));
    assert_eq!(fault.stage, "extension-query");
    assert!(!fault.message.contains("absent"));
    assert!(fault.message.contains("query failed: Broken pipe"));
    assert_eq!(fault.exit_code, None);
}

#[test]
fn present_and_absent_extensions_follow_the_query_result() {
    assert_eq!(
        optional_extension_fault("SHAPE", "no input shapes", ExtensionQuery::Present),
        None
    );
    let fault = record(optional_extension_fault(
        "SHAPE",
        "no input shapes",
        ExtensionQuery::Absent,
    ));
    assert_eq!(fault.stage, "extension");
    assert!(fault.message.starts_with("flamewm: SHAPE absent"));
}

#[test]
fn lost_connection_report_has_no_absent_extension_records() {
    let mut report = StartupReport::default();
    report.push(connection_lost_fault("Broken pipe"));
    let rendered = report.render();
    assert!(!rendered.contains("absent"));
    assert_eq!(rendered.matches("blast radius").count(), 1);
}

#[test]
fn startup_failure_line_names_each_stage() {
    for stage in ["connect", "integrations", "catalog", "become-wm", "scan"] {
        let line = startup_failure_line(stage, "boom");
        assert!(
            line.starts_with(&format!(
                "FLAMEWM-X11-STARTUP-FAIL stage={stage} cause=boom blast_radius="
            )),
            "{line}"
        );
    }
}

#[test]
fn unknown_startup_stage_is_unclassified() {
    assert!(startup_failure_line("paint", "boom").contains("stage=unclassified"));
}

#[test]
fn missing_display_is_named_in_the_connect_cause() {
    let line = startup_failure_line("connect", &connect_cause("Can't parse DISPLAY"));
    assert!(line.contains("cause=DISPLAY unset or unreachable: Can't parse DISPLAY"));
}

#[test]
fn tagged_startup_error_keeps_its_stage_and_cause() {
    let error = startup_stage("catalog", "no apps");
    assert_eq!(
        startup_failure_report(error.as_ref()),
        startup_failure_line("catalog", "no apps")
    );
}

#[test]
fn untagged_startup_error_is_unclassified() {
    let error = std::io::Error::other("boom");
    assert!(startup_failure_report(&error).contains("stage=unclassified cause=boom"));
}
