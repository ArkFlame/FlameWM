# u45 — X11 session start fault-injection test

Scope: staged X11 startup under three fault conditions — no extensions, no RandR,
and a second WM already holding the root `SubstructureRedirect` selection.
This unit adds pure decision functions and `#[cfg(test)]` coverage; it does not
start an X server and does not change production behaviour.

Evidence read: `docs/swarm/units/u25-x11-startup.md`,
`crates/flamewm-wm-x11/src/wm/construct.rs`,
`crates/flamewm-wm-x11/src/lib.rs`,
`crates/flamewm-wm-x11/src/wm/event_loop.rs`,
`crates/flamewm-wm-x11/src/wm/mod.rs`, `crates/flamewm-shell/src/main.rs`.
Route: `flamewm-source-first`, `flamewm-rust-systems`, `flamewm-x11-window-manager`.

## Extracted pure decisions

| Function | File | Input -> outcome |
|---|---|---|
| `missing_extension_fault` | `wm/construct.rs` | extension name + blast radius -> `StartupFault` record |
| `randr_event_fault` | `wm/construct.rs` | `RandrQuery` (present/absent/failed) + selection error -> live events or a root-geometry fallback record |
| `second_wm_fault` | `wm/construct.rs` | root selection `ErrorKind` -> fatal record with exit code 1 on `Access` |
| `randr_resource_fault` | `lib.rs` | RandR resource error -> fallback record |
| `empty_randr_resources` | `lib.rs` | no timestamp, no outputs, no modes -> root-geometry fallback values |

`StartupFault` carries `stage`, the exact operator-facing `message`, and
`exit_code` (`None` for degradations, `Some(1)` for the fatal second-WM case).
`become_wm` and `display_snapshot` now take their decision from these functions
and emit the same messages and fallback values as before; the fatal error is
still propagated to `run_with_hook` and the shell's `exit(1)`.

## Fault matrix covered by `wm/startup_fault_tests.rs`

| Stage / fault | Outcome asserted | Failure record |
|---|---|---|
| RandR present, selection succeeds | no fault | none |
| RandR absent | degrade to root geometry | stage `extension`, "RandR absent" |
| RandR query fails | degrade to root geometry | stage `randr-events`, query error text |
| RandR event selection fails | degrade to root geometry until restart | stage `randr-events`, selection error text |
| Second WM holds root selection (`Access`) | clear error, exit code 1 | stage `root-selection`, `exit_code == Some(1)` |
| Other/no X11 error on root selection | no second-WM fault | none |
| Optional extension missing | degrade | stage `extension`, extension name and blast radius |
| RandR resources unavailable | `(0, [], [])` -> root geometry | stage `randr-resources`, "falling back to root geometry" |

UNBUILT — this unit is forbidden from running `git`, `cargo`, Python, builds or
tests. The orchestrator must compile the crate and run the new test module.

Status: UNBUILT (unverified until orchestrator build)

PARTIAL — all three requested fallback decisions are extracted into pure functions
and the new test file covers every listed stage's failure outcome plus its failure
record, with production messages and fallbacks preserved, but no compiler or test
runner was invoked here, so correctness and visibility of the new module remain
unverified until the orchestrator builds.
