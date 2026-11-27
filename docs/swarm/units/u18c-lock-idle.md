# u18c — Lock clock repaint and notification idle wake (narrow retry)

**Scope:** `crates/flamewm-lock/src/run.rs` only, plus this file. Shared checkout; the
orchestrator commits. RULES: no `git`, no `cargo`, no Python, no builds or tests. `rg` over
`timer`, `tick`, `sleep`, `Duration`, `request_redraw`, `redraw` in the owned file; cross-crate
modules (`run/paint.rs`, `run/tick.rs`, `run/idle.rs`, `run/tests.rs`, `native_time.rs`,
`clock.rs`, `view.rs`, `screensaver.rs`, `power.rs`) were read as evidence only and were not
edited. Every statement below is static source inspection and is **UNBUILT**: no compiler, build
or runtime evidence was collected in this pass.

## Findings

| file:line | Cost | Change | Risk |
| --- | --- | --- | --- |
| `run.rs:63-71,238-240` (idle tick candidate) | One repeating 1 s timer wakes the reactor while nothing visible changes; the clock is minute-resolution. | None. The tick is the only driver of the 4 s power error TTL (`power.rs:259-267`), the 1 s grab/stacking safety net (`HOUSEKEEPING`, `run/tick.rs:91-99`), DPMS-on retries, pointer polling (`XQueryPointer` drains the X socket without fd readiness) and the minute screensaver thresholds. Aligning it to the minute boundary delays visible clock/screensaver transitions by up to 60 s and drops the grab safety net; shortening it adds wakes with no new visible state. | Visual/security — rejected under the identical-visuals rule. |
| `run.rs:264-274` (`PaintKey`) with evidence `run/paint.rs:38-63`, `native_time.rs:6-14,40-59`, `clock.rs:22-36` | A per-second clock repaint would redraw the whole lock frame when seconds are not shown. | None. `LocalTime` carries no seconds, so `PaintKey.time` changes only at a minute boundary; `repaint` returns before projection on an equal key, and a second `view_changed`/`power_changed` equality check returns before `redraw` when the projected strings are equal. `format_clock` renders hour and minute only. | None — already minute-granular; no cut exists that preserves identical visuals. |
| `run.rs:326-379` (frame pacer, recheck at `354-359`) | A fixed sleep that always wrote a wake byte gave one needless reactor turn after Ribbons stopped. | None this pass. The post-sleep flag recheck from u18 was re-read and is present: when the animation stopped during the sleep the thread returns to the condvar without waking; cadence while running is unchanged. | Low — `set_running` still owns the condvar; re-applying a change would risk a dropped frame. |
| `run.rs:164-168,228-237` (cross-thread wake socket) | A worker-side poll would burn CPU at idle. | None. PAM, power and pacer results arrive as bytes on a nonblocking socket pair; the reactor drains all pending bytes per readiness. | None — event-driven. |
| `run.rs:71` (`PREVIEW_POLL`) | A 50 ms polling interval is a short wake. | None. It is consumed only by the short-lived settings preview in `run/preview_mode.rs` (not owned here) to observe `XQueryPointer`/`XQueryKeymap`, which produce no fd readiness; frames there are written only when due. | Visual — removing it would freeze preview dismissal; outside this unit's ownership. |

## Applied edits

None. No code was edited in this pass. The inherited cuts in the owned file (frame-pacer
post-sleep recheck) and the minute-granular repaint gate were re-read and left as they are.

## Verification

- No `git`, `cargo`, Python, build, test or sleep-loop command was run, per the brief; only `rg`,
  `sed`/`wc` and file reads were used. All findings are static source inspection only.
- `run.rs:63-74,164-168,211,228-242,264-274,326-379` and the evidence files were re-read
  immediately before finalizing; no concurrent edit needed to be accounted for.
- **UNBUILT:** there is no new code to compile. The inherited frame-pacer edit remains unbuilt in
  this checkout and needs the orchestrator's build and test pass.

PARTIAL — the narrow retry re-checked every timer, poll, repaint gate and clock path reachable
from `run.rs` and found no wake faster than the visible change that can be cut safely with
identical visuals. The clock key is already minute-granular and `repaint` already redraws only
when the displayed view or power view changes; the only remaining periodic wake is the 1 s tick,
which drives the error TTL, grab safety net, DPMS retry and minute screensaver transitions. Zero
code edits were made, so there is nothing new to build; the inherited edit stays UNBUILT pending
orchestrator verification.

Status: PASS
