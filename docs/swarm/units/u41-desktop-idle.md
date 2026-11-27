# Unit u41 — Desktop app idle wakeups and widget refresh audit

Status: **UNBUILT** — source audit only. No `git`, `cargo`, build, test or runtime measurement was
run (worker constraint); visual equivalence is argued from the current call paths, not measured.

Scope: `crates/flamewm-desktop/src` plus this file. `wallpaper_loader.rs`, `widgets/sync.rs` and
`widgets/render.rs` were re-read only; they are owned by other workers this session. No code file
was edited by this unit: both cuts named by the brief were found already landed in the shared
checkout, so re-applying them would be a duplicate edit, not a cut.

## Inventory: every timer and wake in the desktop app

"Covered" means `UiControllerEvent::Visibility { obscured: true }`, delivered from X11
`VisibilityNotify` (`crates/flamewm-ui-x11/src/lib.rs:157-159`, `:213-214`). "Display off" means
DPMS/blank: the desktop crate has no power-state source, so a blanked but still mapped surface is
indistinguishable from a visible one and no timer is gated on it.

| mechanism | file:line | period | wakeups/s at idle | fires while fully covered | fires when display off |
| --- | --- | --- | --- | --- | --- |
| Widget tick deadline (seconds) | `widgets/sampling.rs:39-55`, `:70-77` | wall-clock second, boundary-aligned at `:71-74` | 1.00 when a seconds/analog clock or Custom widget exists | No — `widgets/lifecycle.rs:37-38` drops the slot, `:55-58` re-drops it on every flush | **Yes** — no display-off state exists |
| Widget tick deadline (minute) | `widgets/sampling.rs:47-54`, `:70-77` | wall-clock minute, boundary-aligned | 0.0167 with Clock/Calendar but no seconds widget | No (same gate) | **Yes** |
| Widget sampler deadline | `widgets/sampling.rs:69`, `:103`; interval at `widgets/mod.rs:139` | 4 s fixed `Instant` cadence | 0.25 with CPU/memory/ring/history/network/custom widget; 0 otherwise | No (same gate; deferred to uncover via `lifecycle.rs:41-47`) | **Yes** |
| Clock/Calendar document refresh | `widgets/render.rs:133-180` (`sync_widgets`, covered guard at `:137-140`, time-only fast path `:156-180`) | driven only by the tick above; no own timer | 0 own | No (covered guard returns before any write) | **Yes** when a clock exists |
| Icon completion wake fd | `main.rs:794-803` | worker completion only | 0 periodic | **Yes** — completion is drained and applied at `main.rs:507-514` even while covered | **Yes** if a completion lands |
| Wallpaper completion wake fd | `main.rs:805-823`, `wallpaper_loader.rs:263` | worker completion only; worker condvar has no deadline | 0 periodic | **Yes** — drained at `:808-821`; projection itself is deferred by the covered branch | **Yes** if a completion lands |
| Wallpaper slideshow / rotation | no source found; `docs/perf-loop/LOG-u80-wallpaper-idle.md:30-33` | none | 0 | n/a — no timer exists to gate | n/a |
| Trash worker completion wake fd | `main.rs:740-762` | worker completion only | 0 periodic | **Yes** | **Yes** |
| MPRIS media widget wake fd | `main.rs:543-574` | D-Bus event only; the provider's retry backoff is consumed by the WM runtime, not here | 0 periodic | **Yes** — `refresh_media` runs from the fd callback | **Yes** |
| Sticky save debounce | `sticky.rs:53` (500 ms), `:114-127`, `:290-318`; armed from `refresh.rs:215` | one-shot per edit burst, retry cap 30 s (`:55`) | ~0 idle; 1 wake 0.5 s after a save becomes pending | **Yes** if a save is pending | **Yes** |
| Downloads settle poll | `downloads.rs:258-266`; `POLL` = 100 ms at `flamewm-downloads-core/src/lib.rs:18`; enabled gate at `main.rs:494-500` | 100 ms only while files are tracked; disarmed immediately when empty (`:259-262`) | 10.00 only during an active organize burst, else 0 | **Yes** (background housekeeping) | **Yes** |
| Profiler report timer | `main.rs:293-305`; interval from `flamewm-profiler/src/lib.rs:80-85` | 60 s default (floor 10 s), opt-in via `FLAMEWM_PROFILE_INTERVAL` | 0.0167 when enabled, else 0 | **Yes** — diagnostic, must stay live | **Yes** |
| Startup heap trim | `main.rs:307-313`; delay at `state.rs:6` | one-shot, 10 s after start | 0 steady-state | **Yes** if still pending | **Yes** |
| Cursor theme retry | `refresh.rs:297-371` | no armed timer; evaluated on settings turns, 5 s → 300 s backoff | 0 periodic | **Yes** on a settings turn | **Yes** |
| Reactor wake aggregation | `flamewm-reactor/src/lib.rs:65-94`, `:504-524`; three owned slots wired at `main.rs:314-321`, `:772`, `sticky.rs:286-288` | none — dispatches bounded by the earliest owner deadline | 0 own | follows each owner above | follows each owner above |

Worst-case steady widget wake rate on an uncovered, visible desktop with a seconds clock and a
CPU widget is 1.25/s (1.00 tick + 0.25 sample). A widget-free desktop is 0/s, matching the
no-clock-read idle dispatch in `flamewm-reactor/src/lib.rs:506-512`.

## Safe cuts with identical visuals (before -> after)

| file:line | wakeups/s before | wakeups/s after | risk |
| --- | --- | --- | --- |
| `widgets/sampling.rs:70-75` (wall-clock boundary alignment via `wall_clock_delay_ms`, `flamewm-reactor/src/lib.rs:432-444`) | 1.00 (or 0.0167 for minute clocks) at an arbitrary phase from `now + 1 s` | 1.00 (or 0.0167) at the displayed second/minute boundary; no extra wake | Already present; no edit made. `epoch_ms` is sampled just after `now`, so the computed `Instant` deadline can be microseconds early; a boundary tick could read the previous displayed value once. Pre-existing, not changed here. |
| `widgets/lifecycle.rs:29-49` (`:38` drops the slot on obscure), `:51-58` (`:55-57` re-checks every flush), `widgets/render.rs:137-140` (no writes while covered) + `main.rs:507-517` (projection deferred) | 1.25/s tick + sample while fully covered | 0/s while fully covered; resumed on uncover by `lifecycle.rs:41-47` | Already present; no edit made. `refresh_after_change` (`lifecycle.rs:83-91`) still arms unconditionally, but its callers are widget pointer/key actions (`widgets/actions.rs:190,217,270,302`), which cannot reach a fully covered surface; the next flush clears the slot. |

No third cut was made. The downloads settle timer and sticky debounce were left alone because
pausing them would delay background housekeeping rather than merely defer an invisible repaint,
so they are not "identical visuals" cuts. The opt-in profiler timer was left alone because its
purpose is to report while the desktop is idle.

Residual observation (no edit): display-off is not observable in this crate, so every non-obscured
wake above still fires on a blanked screen. Adding a power-state source would cross into the
shell/reactor owners and is outside this unit.

## Verification

Source re-read of every cited path. No build, test, formatting check or runtime trace was run;
`git` and `cargo` were forbidden by the worker contract. All numbers are derived from the declared
intervals above, not from a measurement.

PARTIAL — the audit is complete and both candidate cuts (pause widget timers while the surface is
obscured, and align the clock tick to the displayed wall-clock boundary) are already present in the
shared checkout, so this unit made no code edit and claims no new wakeup reduction; everything here
is static source evidence and remains UNBUILT until the orchestrator builds and measures it.
