//! Fault-injection tests for the native → nested → headless DRM init fallback (unit u36c).
//!
//! The fallback rule is tested as a pure mapping from an init outcome to the next backend, and the
//! failure-record side effect is tested against an injected log path. No DRM device, display, event
//! loop or session service is started.

use super::{
    Backend, BackendFallbackOutcome, EXIT_NO_SEAT, NativeAvailability, NativeFallbackDecision,
    NextBackend, append_backend_failure_record, choose_native_fallback,
    native_unavailable_falls_back, next_backend, select_backend,
};

#[test]
fn native_ok_keeps_the_native_backend() {
    assert_eq!(
        next_backend(BackendFallbackOutcome::NativeOk),
        NextBackend::Keep
    );
}

#[test]
fn native_fails_nested_ok_keeps_the_nested_backend() {
    assert_eq!(
        next_backend(BackendFallbackOutcome::NativeFailed),
        NextBackend::Nested
    );
    assert_eq!(
        next_backend(BackendFallbackOutcome::NestedOk),
        NextBackend::Keep
    );
}

#[test]
fn native_fails_nested_fails_goes_headless() {
    assert_eq!(
        next_backend(BackendFallbackOutcome::NativeFailed),
        NextBackend::Nested
    );
    assert_eq!(
        next_backend(BackendFallbackOutcome::NestedFailed),
        NextBackend::Headless
    );
}

/// A native request in a build without the DRM feature must never abort the session: the helper
/// marks it as a fallback case and the pure rule supplies the nested step.
#[test]
fn native_without_drm_feature_falls_through_to_nested() {
    assert!(native_unavailable_falls_back(Backend::Native, false));
    assert!(!native_unavailable_falls_back(Backend::Native, true));
    assert!(!native_unavailable_falls_back(Backend::Nested, false));
    assert_eq!(select_backend(Some("drm")), Backend::Native);
    assert_eq!(
        next_backend(BackendFallbackOutcome::NativeFailed),
        NextBackend::Nested
    );
}

#[test]
fn each_failure_appends_a_record() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-u36c-failure-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    append_backend_failure_record(&path, "drm", "injected drm init failure", true);
    append_backend_failure_record(&path, "nested_winit", "injected nested init failure", true);

    let text = std::fs::read_to_string(&path).expect("injected failure record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "one record per injected failure: {text}");
    assert!(lines[0].contains("stage=\"drm\""), "{}", lines[0]);
    assert!(lines[0].contains("code=subsystem_error"), "{}", lines[0]);
    assert!(
        lines[0].contains("cause=\"injected drm init failure\""),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("fallback_taken=true"), "{}", lines[0]);
    assert!(lines[1].contains("stage=\"nested_winit\""), "{}", lines[1]);
    assert!(lines[1].contains("code=subsystem_error"), "{}", lines[1]);
    assert!(lines[1].contains("fallback_taken=true"), "{}", lines[1]);

    let _ = std::fs::remove_file(&path);
}

/// All 16 combinations of missing DRM device / logind seat / DRM master / input devices have a
/// defined outcome: keep native only when nothing is missing, exit only without a seat, otherwise
/// fall back to the nested winit window.
#[test]
fn every_native_availability_combination_has_a_decision() {
    for drm_device in [false, true] {
        for logind_seat in [false, true] {
            for drm_master in [false, true] {
                for input_devices in [false, true] {
                    let facts = NativeAvailability {
                        drm_device,
                        logind_seat,
                        drm_master,
                        input_devices,
                    };
                    let expected = if !logind_seat {
                        NativeFallbackDecision::ExitWithCode(EXIT_NO_SEAT)
                    } else if drm_device && drm_master && input_devices {
                        NativeFallbackDecision::KeepNative
                    } else {
                        NativeFallbackDecision::NestedWinit
                    };
                    assert_eq!(choose_native_fallback(facts), expected, "{facts:?}");
                }
            }
        }
    }
}

/// The exit code must be distinct from the generic startup failure code (1) and non-zero.
#[test]
fn missing_seat_exit_code_is_clear_and_distinct() {
    assert_ne!(EXIT_NO_SEAT, 0);
    assert_ne!(EXIT_NO_SEAT, 1);
    let facts = NativeAvailability {
        drm_device: false,
        logind_seat: false,
        drm_master: false,
        input_devices: false,
    };
    assert_eq!(
        choose_native_fallback(facts),
        NativeFallbackDecision::ExitWithCode(EXIT_NO_SEAT)
    );
}

/// A failed init stage maps onto exactly the prerequisite it could not obtain.
#[test]
fn failed_stage_names_the_missing_prerequisite() {
    assert!(!NativeAvailability::from_failed_stage("seat").logind_seat);
    let drm = NativeAvailability::from_failed_stage("drm");
    assert!(!drm.drm_device && !drm.drm_master && drm.logind_seat && drm.input_devices);
    assert!(!NativeAvailability::from_failed_stage("outputs").drm_device);
    assert!(!NativeAvailability::from_failed_stage("libinput").input_devices);
    assert_eq!(
        choose_native_fallback(NativeAvailability::complete()),
        NativeFallbackDecision::KeepNative
    );
}
