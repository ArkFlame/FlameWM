# Unit p64-xwayland-missing
STATUS: DONE

Crate flamewm-wm-wayland xwayland.rs: Xwayland binary missing, crashes, or exits: session continues Wayland-only, one report, restart bounded. Test the pure decision function.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-wm-wayland/src/xwayland.rs:324-357` | X11-unavailable handling existed (missing binary in `main.rs`, `XWaylandEvent::Error` and WM-start errors here) but the survive / one-report / bounded-restart contract was implicit and untestable without an X server. | Added pure `XWAYLAND_START_LIMIT`, `XwaylandFailureDecision { continue_wayland_only, report_failure, restart }` and `xwayland_failure_decision(attempts, reported)` beside the other pure helpers; restart is bounded by the single-start limit. | UNBUILT - brief forbids cargo/builds; pure data only, no I/O or ownership change. |
| `crates/flamewm-wm-wayland/src/xwayland.rs:396` | `xwayland_unavailable` reported and cleared autostart unconditionally, with no single decision owner. | It now applies `xwayland_failure_decision(XWAYLAND_START_LIMIT, false)`: one report, autostart cleared to keep the Wayland-only session, plus a `debug_assert!` that no respawn occurs. Existing wrapper detail tokens and printed message unchanged. | UNBUILT - brief forbids builds; existing failure-path behavior is identical. |
| `crates/flamewm-wm-wayland/src/xwayland.rs:1207` | No test covered the pure failure contract; only struts, stacking, material and selection were tested. | Added `xwayland_failure_decision_tests` covering missing/crash/exit sharing one decision, no duplicate report, and the bounded restart budget including `u32::MAX`. | UNBUILT - pure assertions only; no display, spawn or session started. |

PASS - XWayland binary missing, crash, and early exit now share one pure, testable decision: keep the Wayland-only session, emit at most one failure record, and never respawn past the single-start budget. Smithay reference (`.vendor/smithay-master/src/xwayland/xserver.rs:381-399`) confirms its source self-disables after `Ready`/`Error`, so no restart loop exists. Every edit is UNBUILT because the brief forbids cargo, builds and tests; compilation and the new unit tests remain for the orchestrator's batch build. Visuals untouched and no gate loosened.

