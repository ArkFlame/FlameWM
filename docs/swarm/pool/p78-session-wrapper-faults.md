# Unit p78-session-wrapper-faults
STATUS: DONE

scripts/flamewm-session-wrapper and scripts/test-wrapper-faults: read both; list each fallback (no Wayland backend, no X, no GPU, missing binary, bad env) and check the fault script covers it; add missing cases to the script (bash, no python). Do not run it.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| scripts/test-wrapper-faults:15-19 | Header case list omitted the five named fallbacks. | Documented cases 5-9. | UNBUILT: comment only, no runtime effect. |
| scripts/test-wrapper-faults:80-91 | chain_cause (wrapper 86-103) was not extracted, so no-X, missing-binary and bad-env causes were untestable. | Extract chain_cause verbatim with the same awk pattern as the other helpers. | UNBUILT: new extraction anchor must keep matching wrapper text. |
| scripts/test-wrapper-faults:127-128 | Extracted chain_cause was not sourced. | Source the new include after run_safe_mode. | UNBUILT: script not run per brief. |
| scripts/test-wrapper-faults:256-265 | no Wayland backend fallback uncovered (wrapper 291-292 maps libwayland-* to nested_wayland+wayland_lock). | Case 5 asserts non-zero, the nested_wayland degraded label and x11 works. | UNBUILT: script not run per brief. |
| scripts/test-wrapper-faults:267-276 | no GPU fallback uncovered (wrapper 283-284 maps libEGL/libGLESv2/libGL). | Case 6 asserts non-zero, the wayland_gpu_renderer label and x11 works. | UNBUILT: script not run per brief. |
| scripts/test-wrapper-faults:278-286 | no X fallback uncovered; case 4 kept DISPLAY=:99 and only lacked a WM. | Case 7 unsets DISPLAY and checks chain_cause appends +no_display. | UNBUILT: minimal-mode X-socket/Xvfb branch still not directly exercised. |
| scripts/test-wrapper-faults:288-302 | missing binary fallback uncovered (wrapper 89-90). | Case 8 checks rc 127/126 map to binary_missing / binary_not_executable. | UNBUILT: script not run per brief. |
| scripts/test-wrapper-faults:304-326 | bad env fallback uncovered (wrapper 95-101). | Case 9 checks +runtime_dir, +preflight_fatal and +degraded_components suffixes. | UNBUILT: script not run per brief. |

PASS — All five named fallbacks (no Wayland backend, no X, no GPU, missing binary, bad env) are now asserted by cases 5-9 of scripts/test-wrapper-faults, with chain_cause extracted so the no-X, missing-binary and bad-env cause labels are reachable. No production wrapper code changed; the test script was deliberately not run per the brief; every edit is UNBUILT pending the orchestrator's checks.
