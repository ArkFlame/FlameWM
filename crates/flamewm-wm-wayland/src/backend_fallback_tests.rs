//! Fault-injection tests for the native → nested → headless DRM init fallback (unit u36c).
//!
//! The fallback rule is tested as a pure mapping from an init outcome to the next backend, and the
//! failure-record side effect is tested against an injected log path. No DRM device, display, event
//! loop or session service is started.

use super::{BackendFallbackOutcome, NextBackend, append_backend_failure_record, next_backend};

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
