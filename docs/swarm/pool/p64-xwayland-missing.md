# Unit p64-xwayland-missing
STATUS: DONE

Crate flamewm-wm-wayland xwayland.rs: Xwayland binary missing, crashes, or exits: session continues Wayland-only, one report, restart bounded. Test the pure decision function.

## Result
STARTED

- crates/flamewm-wm-wayland/src/xwayland.rs:324-357 | Pure policy model (`XWAYLAND_START_LIMIT`, `XwaylandFailureDecision`, `xwayland_failure_decision`) already present and correct; it encodes continue-Wayland-only, one report, and a bounded restart via saturating arithmetic. | No production edit; requirement already satisfied in the shared checkout. | UNBUILT (no build/test run by worker).
- crates/flamewm-wm-wayland/src/xwayland.rs:1207-1236 | `xwayland_failure_decision_tests` already tests the pure decision function: one report, no restart once the limit is consumed, and a bounded budget for any attempt count. | No new test edit; the redundant duplicate appended earlier in this unit was removed. Existing tests left byte-identical. | UNBUILT.
- docs/swarm/pool/p64-xwayland-missing.md:2,6-13 | Unit was OPEN with only STARTED recorded; an earlier row set in this unit was inaccurate. | STATUS set to DONE and result rows corrected to match the final source state. | UNBUILT (documentation only).

PASS: The requested behavior and its pure-decision test were already present in the shared checkout at xwayland.rs:324-357 and 1207-1236 respectively; no production or test edit was required, and no unrelated work was changed. Build and test execution are out of scope for this worker unit, so every item is UNBUILT and must be verified by the orchestrator's batch build/test.
