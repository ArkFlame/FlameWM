# Unit p67-lock-fail-safe
STATUS: DONE

Crate flamewm-lock: failure to render, missing PAM, missing background: still shows a usable lock (solid colour) and never unlocks on error. Test the pure fallback decision.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-lock/src/wayland.rs:604-648`, `crates/flamewm-lock/src/run/tick.rs:67-85` | Audit: a refused/ended lock returns `LockError::Refused` and is recorded; paint failures are logged and retried; the only unlock flag set is in `poll_outcome`/`tick` after `LockState::finish` returns true, which is reserved for `Outcome::Unlocked`. No error path unlocks. | None — behaviour already correct; codified by the new pure decision below. **UNBUILT** | None. |
| `crates/flamewm-lock/src/document.rs:96-123` | Audit: `background_image` already falls back to `reference_navy` (or a flat configured colour) when decoding fails, so a missing/undecodable background still yields a fully opaque usable lock. | None — no edit; visuals unchanged. **UNBUILT** | None. |
| `crates/flamewm-lock/src/fail_safe.rs:1-40`, `crates/flamewm-lock/src/lib.rs:7` | No single pure function described the combined fail-safe invariant; the three failure cases were only implicit across the two presenters. | Added pure `Faults`/`Fallback`/`decide` and exported the module; added 3 unit tests covering all 8 fault combinations, each single failure, and the healthy case. **UNBUILT** | Additive pure code and tests only; no runtime path changed, no pixels changed, no gate loosened. |

Static audit with `rg`/`Read` only: no `git`, `cargo`, build, test, Python or sleep-loop command was run. Reference precedent: `.vendor/sddm-develop/src/daemon/Greeter.cpp:94-101` falls back to the default theme when the configured greeter resource is missing rather than skipping the greeter; `.vendor/sddm-develop/src/daemon/Display.cpp:162` stops the display on greeter failure instead of opening the session. Both match the coded invariant: degrade to a usable lock, never unlock on error.

PASS — the three failure paths already keep the session covered and never unlock: render failures keep the prior frame and are retried, PAM loss maps to `Status::Unavailable` with `finish() == false`, and a missing background falls back to solid navy. The new pure `fail_safe::decide` codifies the decision (`show_lock` always true, `unlock` always false, solid colour on any fault) and its tests cover every fault combination. All edits are UNBUILT because the brief forbids builds; the orchestrator must compile and test `flamewm-lock` before treating this slice as verified.
