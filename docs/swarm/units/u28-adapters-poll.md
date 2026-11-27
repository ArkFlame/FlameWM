# u28 — Tray, audio, network and calendar adapter polling audit

**Status: UNBUILT.** No `git`, `cargo`, Python, build or test command was run in this shared-checkout
unit; the orchestrator builds, tests and commits after the batch lands. Costs below are code-path
and wake-count budgets read from source, not fresh runtime samples.

## Owned files (listed before any edit)

- `crates/flamewm-integrations-linux/src/mpris.rs` — session-bus media adapter; owns the only
  timer that keeps retrying while the host is idle (reconnect wake).
- `docs/swarm/units/u28-adapters-poll.md` — this audit record.

`icon_service.rs`, `launcher_entry.rs` and `notifications.rs` are owned by other workers and were
not read for edits or touched. No other file under `crates/flamewm-integrations-linux/src` is owned
or edited by this unit.

## Audit readback (all of `crates/flamewm-integrations-linux/src`, excluding the three files above)

| Area | Evidence | Disposition |
| --- | --- | --- |
| Tray | `audio_helper.rs:223-255` reader blocks on helper stdout lines and reaps the child on EOF; `audio_helper.rs:338,402` sleeps are test fixtures only. No tray-side polling in this crate (`notifications.rs` is another owner). | No owned poll to cut. |
| Audio | `pulse.rs:39-49,71-97` uses Pulse subscriptions on one threaded mainloop; `pulse.rs:604,651` arms the retry timer only after a disconnect, parks it at `RETRY_TIMER_SEED` (86 400 s) while available, and shared `BackoffConfig` (`flamewm-integrations-core/src/pulse.rs:49-56`) ends after 8 attempts, so no idle wake remains. `pulse.rs:884` sleep is a test fixture. | No owned poll to cut. |
| Network | `network_slot.rs:16-17,85-101` uses a never-ready socketpair while unavailable; `network_manager/mod.rs:316-330` issues `GetAll` only from an event-triggered reconcile gated by `reconcile_pending`; `on_ready` (`:369`) drains native D-Bus events only. | No owned poll to cut. |
| Calendar | No calendar adapter exists in this crate; `flamewm-shell/src/calendar.rs` is outside this unit's ownership. | Ownership boundary; untouched. |
| Media / MPRIS (tray+audio status) | `mpris.rs:114-154,236-269` retries a missing session bus on a reactor-armed timer (`LinuxIntegrationRuntime::next_retry_in`, `lib.rs:173-178`) forever at a 30 s cap after its 255-attempt budget. | **One cut applied** below: fast early cadence retained, steady idle interval lengthened. |
| Device watch | `device_watch.rs:74-113` blocks in `poll` with no timeout while quiet; the 300 ms deadline exists only to coalesce a uevent/mount burst. | Already event-driven; changing `QUIET` would change visible sidebar reload timing. Rejected. |
| Icon theme | `icon_theme/lookup.rs:451-476` throttles mtime checks to one per 5 s on lookup; the caller is `icon_service.rs` (another owner) and a longer cooldown would delay visible theme reloads. | Rejected. |

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-integrations-linux/src/mpris.rs:114-154,895-918` | With no session bus, the reactor wakes every 30 s forever (255 fast attempts at `RECONNECT_INITIAL_MS`/`RECONNECT_MAX_MS`, then `unwrap_or(RECONNECT_MAX_MS)`), so an idle host pays 120 wakes/hour plus a D-Bus connect attempt for an absent bus. | **EDIT (UNBUILT):** added `FAST_ATTEMPTS = 7` and `IDLE_RETRY_MS = 120_000`; `ReconnectBackoff::next_delay` passes `FAST_ATTEMPTS` to the shared exponential helper and returns the 120 s idle interval once that budget is spent. The first seven delays are byte-for-byte the old schedule (`1,2,4,8,16,30,30` s), and `reset()` still restores the fast schedule after any successful connect, so active-state snapshots and media actions are unchanged. Updated the schedule doc comment and the budget-spent unit test to assert the new idle interval plus a reset back to the initial delay. | Medium until measured: a session bus that appears after the fast budget is now discovered within 120 s instead of 30 s; snapshots and generated media state are unchanged, but recovery latency after a long absence is longer by design. No runtime sample was collected here; the shared backoff helper (`flamewm-integrations-core/src/backoff.rs:6-24`) returns `None` once the budget is spent, so the idle branch is deterministic. |

## Verification

- No `cargo`, `git`, tests, builds, Python or sleep loops were run in this unit, per the brief. Both
  edited files are **UNBUILT**; the orchestrator's build and test run is still owed.
- Source readback only: the shared helper returns `None` when `attempts >= maximum_attempts` and
  clears `pending`, so every call after the seventh returns `IDLE_RETRY_MS`; `reset()` restores
  `attempts = 0, pending = true`. `retry_if_due` (`:256-261`) still calls `try_connect` once the
  published deadline passes, and `next_retry_in` (`:265-269`) still publishes that deadline to the
  reactor, so no wake can be lost or duplicated by the schedule change.
- The seven-delay regression test and the reset test keep their original expectations; only the
  budget-spent test was rewritten for the idle interval.

PARTIAL — one of the at most three permitted cuts was applied: the MPRIS absent-bus retry timer now
keeps its fast 7-step schedule and drops to a 120 s idle wake instead of waking every 30 s forever.
Tray, audio, network and calendar adapters in this crate are already signal/event-driven, and the
other candidate intervals (device-watch quiet window, icon-theme mtime cooldown, Pulse one-day
park) were rejected because lengthening them would change visible timing or buy no idle wake. The
slice is marked UNBUILT and awaits the orchestrator's build, tests and commit.
