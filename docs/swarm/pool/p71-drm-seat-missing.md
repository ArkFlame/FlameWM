# Unit p71-drm-seat-missing
STATUS: DONE

Crate flamewm-wm-wayland drm and session: no DRM device, no logind seat, no DRM master, no input devices: fall back to winit nested or report and exit with a clear code; never panic. Test the pure fallback chooser.

## Result
STARTED

- (crates/flamewm-wm-wayland/src/main.rs:234-297, finding: native init already returned typed errors, but there was no pure chooser over the four missing-resource facts and no clear exit path for a missing seat, change: added NativeAvailability (drm_device/logind_seat/drm_master/input_devices), NativeFallbackDecision (KeepNative/NestedWinit/ExitWithCode), EXIT_NO_SEAT=2 and pure choose_native_fallback; UNBUILT, risk: no seat now exits 2 instead of falling back to nested in drm builds; no build/test run)
- (crates/flamewm-wm-wayland/src/main.rs:861-889, finding: the native error arm always fell through to nested, so a missing logind seat could only hide in a headless compositor, change: wired from_failed_stage + choose_native_fallback into the native startup error arm, keeping the nested/headless chain for hardware gaps and returning Ok(2) with a clear line for a missing seat; UNBUILT, risk: behaviour change for explicit native requests without a seat; no build/test run)
- (crates/flamewm-wm-wayland/src/backend_fallback_tests.rs:7-11,88-136, finding: existing tests covered init outcomes but not the four availability facts, change: added all 16 combinations plus exit-code and failed-stage mapping tests; UNBUILT, risk: test compile unverified)
- (reference .vendor/smithay-master/anvil/src/udev.rs:227-233 and .vendor/cosmic-epoch/cosmic-comp/src/backend/mod.rs:20-45, finding: upstream returns/records session errors rather than panicking and falls back from a failed primary backend to winit, change: followed that precedent with a typed decision instead of a panic and kept the nested fallback; UNBUILT, risk: reference only, no runtime proof)
- (native panic audit crates/flamewm-wm-wayland/src/drm/{init,device,input,mod}.rs and logind.rs, finding: no unwrap/expect/panic in production missing-resource paths; errors already propagate, change: none needed there; UNBUILT, risk: static inspection only)

PASS: a pure chooser now maps every combination of missing DRM device, seat, master and input devices to keep-native, nested winit or a distinct exit-2 path; it is wired into the native startup error arm with no panic path, and tests cover all 16 combinations. All edits are UNBUILT: the orchestrator runs builds and tests after the batch lands, so compile success and runtime behaviour remain unverified.
