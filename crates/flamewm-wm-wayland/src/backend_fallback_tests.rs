//! Fault-injection tests for the native → nested → headless DRM init fallback (unit u36c).
//!
//! The fallback rule is tested as a pure mapping from an init outcome to the next backend, and the
//! failure-record side effect is tested against an injected log path. No DRM device, display, event
//! loop or session service is started.

use super::{
    Backend, BackendFallbackOutcome, EXIT_NO_SEAT, NativeAvailability, NativeFallbackDecision,
    NextBackend, Stage, append_backend_failure_record, append_headless_entry_record,
    append_xwayland_unavailable_record, choose_native_fallback, native_init_failure_detail,
    native_unavailable_falls_back, next_backend, seat_failure_detail, select_backend,
    stage_failure_fallback,
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

/// A missing logind seat exits without a fallback, so its failure record must say `fallback_taken=false`.
/// DRM and output gaps still fall through to the nested window and record `true`.
#[test]
fn seat_failure_takes_no_fallback() {
    assert!(!stage_failure_fallback(Stage::Seat));
    assert!(stage_failure_fallback(Stage::Drm));
    assert!(stage_failure_fallback(Stage::Outputs));
    assert!(!stage_failure_fallback(Stage::Libinput));
}

/// Entering the headless step leaves exactly one record, stage `headless`, with `fallback_taken=true`.
#[test]
fn headless_entry_appends_a_record() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-g2-headless-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    append_headless_entry_record(&path);

    let text = std::fs::read_to_string(&path).expect("headless entry record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one headless record: {text}");
    assert!(lines[0].contains("stage=\"headless\""), "{}", lines[0]);
    assert!(lines[0].contains("fallback_taken=true"), "{}", lines[0]);

    let _ = std::fs::remove_file(&path);
}

/// An inactive logind session fails closed with `EXIT_NO_SEAT`: the seat record must keep the
/// `reason=inactive_after_activate` cause while saying no fallback was taken. This mirrors the
/// `stage_line(Stage::Seat, Fail, detail)` write with the same pure fallback decision.
#[test]
fn inactive_session_record_fails_closed_without_fallback() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-p100-inactive-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let detail = "reason=inactive_after_activate: session /org/freedesktop/login1/session/_3 is still not active";
    append_backend_failure_record(
        &path,
        Stage::Seat.name(),
        detail,
        stage_failure_fallback(Stage::Seat),
    );

    let text = std::fs::read_to_string(&path).expect("inactive session record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one seat record: {text}");
    assert!(lines[0].contains("stage=\"seat\""), "{}", lines[0]);
    assert!(
        lines[0].contains("cause=\"reason=inactive_after_activate"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("fallback_taken=false"), "{}", lines[0]);

    let _ = std::fs::remove_file(&path);
}

/// XWayland is optional: a spawn failure leaves one `xwayland` record that keeps the Wayland socket,
/// compositor and input in `still_works`, degrades only X11 clients and autostart, and marks the
/// degraded X11 path as the fallback that was taken.
#[test]
fn xwayland_failure_appends_a_record() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-p100-xwayland-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    append_xwayland_unavailable_record(&path, "injected xwayland spawn failure");

    let text = std::fs::read_to_string(&path).expect("xwayland failure record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one xwayland record: {text}");
    assert!(lines[0].contains("stage=\"xwayland\""), "{}", lines[0]);
    assert!(lines[0].contains("code=subsystem_error"), "{}", lines[0]);
    assert!(
        lines[0].contains("cause=\"injected xwayland spawn failure\""),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains(
            "still_works=\"process,session_log,wayland_socket,compositor_backend,hardware_outputs,input_devices\""
        ),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains("degraded=\"x11_clients,autostart\""),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("fallback_taken=true"), "{}", lines[0]);

    let _ = std::fs::remove_file(&path);
}

/// Row 3: a native GL/EGL failure is wrapped by `start_native` as a Drm stage failure, so its record
/// must keep the EGL cause and say the nested fallback was taken. The pure chooser then picks the
/// nested window. No EGL display, device or event loop is created.
#[test]
fn native_gl_failure_records_a_drm_fallback() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-p100-gl-native-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let cause = "EGLDisplay::new failed: EGL_BAD_ALLOC";

    append_backend_failure_record(
        &path,
        Stage::Drm.name(),
        cause,
        stage_failure_fallback(Stage::Drm),
    );

    let text = std::fs::read_to_string(&path).expect("native GL failure record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one native GL record: {text}");
    assert!(lines[0].contains("stage=\"drm\""), "{}", lines[0]);
    assert!(lines[0].contains("code=subsystem_error"), "{}", lines[0]);
    assert!(lines[0].contains(cause), "{}", lines[0]);
    assert!(lines[0].contains("fallback_taken=true"), "{}", lines[0]);
    assert_eq!(
        choose_native_fallback(NativeAvailability::from_failed_stage("drm")),
        NativeFallbackDecision::NestedWinit
    );

    let _ = std::fs::remove_file(&path);
}

/// Row 4: a nested winit GL/EGL failure leaves one `nested_winit` record with `fallback_taken=true`,
/// matching the `record_backend_failure` call in `start_nested_or_headless`. The test only writes to
/// an injected log path; no window surface, EGL context or session service is started.
#[test]
fn nested_gl_failure_records_a_winit_fallback() {
    let path = std::env::temp_dir().join(format!(
        "flamewm-p100-gl-nested-record-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let cause = "init_winit failed: no EGL surface (software renderer)";

    append_backend_failure_record(&path, "nested_winit", cause, true);

    let text = std::fs::read_to_string(&path).expect("nested GL failure record log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one nested GL record: {text}");
    assert!(lines[0].contains("stage=\"nested_winit\""), "{}", lines[0]);
    assert!(lines[0].contains("code=subsystem_error"), "{}", lines[0]);
    assert!(lines[0].contains(cause), "{}", lines[0]);
    assert!(lines[0].contains("fallback_taken=true"), "{}", lines[0]);

    assert_eq!(
        next_backend(BackendFallbackOutcome::NestedFailed),
        NextBackend::Headless
    );

    let _ = std::fs::remove_file(&path);
}

/// Row 3 notice: an EGL/GL init failure is labeled `egl_unavailable` before it reaches the Drm
/// record, while an unrelated DRM failure keeps its original cause. Pure string work, no GPU.
#[test]
fn native_init_failure_labels_egl_and_keeps_other_causes() {
    let egl = native_init_failure_detail("EGLDisplay::new failed: EGL_BAD_ALLOC");
    assert!(egl.starts_with("egl_unavailable,error="), "{egl}");
    assert!(egl.contains("EGL_BAD_ALLOC"), "{egl}");
    assert_eq!(
        native_init_failure_detail("cannot open render node"),
        "cannot open render node"
    );
    assert!(stage_failure_fallback(Stage::Drm));
}

/// Row 9 notice: the inactive-after-activate refusal is marked fail-closed in the seat detail and
/// the seat stage still takes no fallback. Other logind reasons keep their original detail.
#[test]
fn seat_failure_marks_inactive_as_fail_closed() {
    let inactive = seat_failure_detail(&"reason=inactive_after_activate: session still inactive");
    assert!(
        inactive.starts_with("fail_closed,reason=inactive_after_activate,"),
        "{inactive}"
    );
    assert_eq!(
        seat_failure_detail(&"reason=greeter: another session is active"),
        "reason=greeter: another session is active"
    );
    assert!(!stage_failure_fallback(Stage::Seat));
    assert_eq!(
        choose_native_fallback(NativeAvailability::from_failed_stage("seat")),
        NativeFallbackDecision::ExitWithCode(EXIT_NO_SEAT)
    );
}
