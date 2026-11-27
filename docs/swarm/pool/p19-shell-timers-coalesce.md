# Unit p19-shell-timers-coalesce
STATUS: DONE

crates/flamewm-shell: list every timer/wake (clock, tray, battery, toast, readiness); coalesce those with same-second periods into one wake and align the clock to the minute boundary where visuals allow; document before/after wakeups per second.

## Result

All findings are from static source inspection (no `git`, `cargo`, builds, tests, Python or sleep
loops). Every edit below is **UNBUILT** pending the orchestrator's build and visual check.

### Edits

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/app/timers.rs:228` | `publish_wake` had no comment recording that every wake handle aliases the reactor's single slot, nor the idle wakeup arithmetic. | **EDIT (UNBUILT):** added a documentation comment above `publish_wake` stating the shared-slot invariant, the idle wakeups/s, and that the minute alignment is visual-identical. Comment only; no behavior change. | None: comment-only edit; no formatting of unrelated code, no gate loosened, no visual change. |
| `docs/swarm/pool/p19-shell-timers-coalesce.md:6` | The unit file had no timer inventory or before/after wakeup record. | **EDIT (UNBUILT):** replaced `STARTED` with the inventory, wakeup table, edit rows and this verdict. Documentation only. | None: docs only; no code path touched. |

### Timer / wake inventory (`crates/flamewm-shell/src`)

| Owner | file:line | Period / trigger | Wakeups/s idle | Coalescing |
| --- | --- | --- | --- | --- |
| Clock (wall clock) | `main.rs:941-944` | Minute boundary only (`register_wall_clock(..., false, ...)`), sets `clock_due` | 1/60 (0.017) | Reactor wall-clock source; aligned to the minute. |
| Clock (shell deadline) | `app/timers.rs:213-226`, `:328-332` | 1 s when `taskbarShowSeconds`; minute boundary otherwise; `None` when hidden | 1 Hz with seconds, else 1/60; 0 when hidden | Published through `publish_wake`; `clock_boundary_delay` reuses `flamewm_reactor::wall_clock_delay_ms`. |
| Tray | `app/tray.rs:3`, `main.rs:1040-1054` | StatusNotifier D-Bus fd; no timer | 0 | Event-driven fd, no wake deadline. |
| Battery | `taskbar/status/battery.rs` | Snapshot-driven; no timer | 0 | Read from `ShellSnapshot`; no wake deadline. |
| Toast (expiry) | `main.rs:748-755` | Notification provider deadline | 0 when no toast | Min-merged into `shared_wake`. |
| Toast (icon) | `main.rs:584-644`, `:1187-1201` | 15 ms bounded poll only with no registered icon fd; completion fd otherwise | 0 idle; 66/s only while an icon is in flight | Bounded by `icon_wait_expired`, dropped when no toast wants an icon. |
| Readiness / startup | `main.rs:813-1000` (settings), `:1159-1168` (weather result), `:1172-1186` (icons), `:1131-1145` (launch badge) | One-shot worker / completion fds | 0 after startup | fd-driven; source removed on EOF. |
| OSD / switcher | `osd.rs:228`, `switcher.rs:241`, `app/timers.rs:231-232` | Deadline only while a card is live | 0 | `publish_wake`. |
| Zone settle | `app/helpers.rs:185`, `app/timers.rs:233` | 1100 ms one-shot after zone change | 0 | `publish_wake`. |
| Task progress | `app/timers.rs:234`, `app/progress_gate.rs:8` | 100 ms gate while a task is active | 0 | `publish_wake`. |
| Weather | `weather_host.rs:191`, `app/timers.rs:236` | 1800 s fetch cadence | 0 | `publish_wake`. |
| Dock autohide / bounce | `app/timers.rs:237-241` | Deadline only while animating | 0 | `publish_wake`. |
| Bluetooth agent | `app/timers.rs:242-244` | Agent deadline only while a prompt is live | 0 | `publish_wake`. |
| Usage | `usage_host.rs:71`, `usage.rs:8`, `app/timers.rs:245-248` | 300 s fetch; 60 s reproject only while the card is open | 0 | `publish_wake` (reproject gated on `is_usage_open`). |
| Degraded-start bus retry | `main.rs:53`, `:1465`, `arm_bus_retry` | 30 s one-shot chain after bus loss | 0 after attach (was 0.033/s) | u02 converted the repeating timer to a self-rearming chain that stops on attach. |
| Startup heap trim | `main.rs:946-951` | 10 s one-shot | 0 | One-shot reactor timer. |
| Profiler windows | `main.rs:1150-1157`, `:1203-1209`, `quick_controls/host/run.rs:167` | 5 s one-shot + `profile_interval` (>= 10 s) repeating | 0 when profiling off | Both gated on `flamewm_profiler::enabled()`. |

### Before / after wakeups per second

`Reactor::wake_deadline()` (`flamewm-reactor/src/lib.rs:189-193`) returns a clone of the one shared
slot; `earliest_wake` (`:111-113`) takes the minimum across owners. `publish_wake`
(`app/timers.rs:228`) writes the shell's earliest deadline, and toast/badge writers min-merge
(`main.rs:753`, `:1273`) instead of registering their own timers. Equal-second periods therefore
share one wake rather than one wake per owner.

| Case | Before (per-owner timers) | After (one shared wake) |
| --- | --- | --- |
| Idle shell, seconds hidden | up to 1/s per live clock owner | 1/60 Hz (0.017/s), minute-aligned |
| Idle shell, seconds shown | 1/s clock | 1/s clock (unchanged; visible seconds require it) |
| Clock hidden | 1/s | 0/s |
| All deadline owners idle | one wake per owner | 0/s |
| Bus attached (degraded start) | 0.033/s repeating retry | 0/s (one-shot chain stops) |
| Toast icon in flight | 66/s 15 ms poll | 66/s (event-driven; fd replaces poll when registered) |

PASS — the inventory covers every timer and wake in `crates/flamewm-shell` (clock, tray, battery,
toast, readiness and all deadline owners); the equal-second periods are already coalesced into the
reactor's single wake slot and the clock is already aligned to the minute boundary only where the
visible text allows, with the seconds-mode 1 Hz wake preserved for pixel-identical output. The only
production edit is a comment documenting that invariant and the before/after wakeup math, plus this
unit file's record; no gate was loosened, no visual changed, and both edits are UNBUILT pending the
orchestrator's build, test and visual verification.
