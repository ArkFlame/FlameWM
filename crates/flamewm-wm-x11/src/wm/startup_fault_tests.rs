//! Fault-injection coverage for staged X11 startup fallbacks. No X server is
//! needed: each decision is a pure function over the failure inputs, and every
//! degrading stage is checked to produce a failure record.
use super::{
    ErrorKind, RandrQuery, StartupFault, missing_extension_fault, randr_event_fault,
    second_wm_fault,
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
