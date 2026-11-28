# Unit p86-clock-notification-timers
STATUS: PARTIAL

Crates flamewm-shell clock and notification center: clock tick aligned to the minute boundary when seconds are hidden, no tick when the panel is hidden; notification expiry one coalesced deadline. Verify and fix deviations. Max 3 edits.

## Result

Evidence is static source inspection (`rg`/Read) only: no `git`, `cargo`, build, test runner, Python
or sleep loop. Every edit below is **UNBUILT** pending the orchestrator's build and visual check.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/app/timers.rs:213-233` | `rearm_clock` anchored the monotonic deadline to the `now` captured by `tick` before `clock_boundary_delay` read the wall clock, then added a 1 ms fudge. The deadline could land just before the displayed minute/second and paint the previous value until the next boundary — the same residual p02 fixed in the desktop widget owner. | Anchor `Instant::now()` after the wall-clock epoch read inside the shown-only closure and drop the 1 ms fudge; rename the now-unused parameter to `_now`. A hidden clock still re-arms to `None` (no epoch read, wake or repaint). | UNBUILT. Worker contract forbids cargo, build, test and runtime checks. The shift is sub-millisecond; visuals stay pixel-identical apart from removing a stale frame. |
| `crates/flamewm-shell/src/app/timers.rs:219-224` | `shown` checked only `taskbarShowClock`. With `taskbar.enabled=false`, `apply_taskbar_enabled` (`runtime/panels.rs:66-80`) hides every panel surface, yet the shell deadline stayed armed, so the clock still computed a boundary deadline and reprojected/redrew hidden panel documents up to 1/60 Hz. | Gate `shown` on `taskbar_enabled` via `settings_cache.as_ref().is_none_or(taskbar_enabled)` before the `taskbarShowClock` check. `taskbar.enabled` is a settings key, so re-enabling bumps `settings_revision`, flips `clock_settings_changed` and re-arms the deadline on the next tick without any new plumbing. | UNBUILT. No gate loosened, no timer added; the suppressed work touched only hidden surfaces, so rendered pixels are unchanged. Minute/second alignment and the single shared wake slot are untouched. |
| `crates/flamewm-shell-core/src/clock.rs:20-48` and `crates/flamewm-reactor/src/lib.rs:432-444` (read only) | Both owners already align to the next minute (or second) boundary and return the full interval on an exact boundary; the shell reuses the reactor rule through `clock_boundary_delay` (`:332-339`, now `:337-344`). | No edit: authoritative boundary rule reused as-is. | None. |
| `crates/flamewm-shell/src/main.rs:993-1001` and `crates/flamewm-shell/src/app/timers.rs:118-135` (read only) | One minute-only reactor wall-clock timer drives `clock_due`; the shell deadline is `None` while the clock is hidden and the hidden tick consumes no formatting or panel repaint. The always-on reactor timer is a deliberate p46 decision (needed for suspend realignment before settings arrive). | No edit: already compliant; the new `taskbar.enabled` gate closes the remaining hidden-panel case in the shell deadline owner. | None. |
| `crates/flamewm-integrations-linux/src/notifications.rs:152-158,331-357,398-399` and `crates/flamewm-shell/src/main.rs:716-721,792-807` (read only) | One coalesced expiry deadline: `earliest_deadline`/`next_deadline` take the min over active toasts, `expire` skips until that deadline, and `publish_toast_deadline` keeps the earlier of the existing wake and the toast deadline. No notification path registers its own reactor timer. | No edit: already compliant with "one coalesced deadline". | None. |

Completion pass: re-read each source file before editing and confirmed the read-only findings above.
One deviation remained: edit #1's `+ std::time::Duration::from_millis(1)` fudge was still present in
`rearm_clock` (the anchor had been moved after the wall-clock read and the parameter renamed, but the
fudge removal was not applied). Removed it so source matches the recorded Result. Re-verified that
`taskbar_enabled` is in scope via `use super::*` (`app/helpers.rs:48`), that the reactor boundary rule
returns the full interval on an exact boundary (`reactor/src/lib.rs:436-444`), and that notification
expiry uses one coalesced min deadline. No further edits were needed; total production edits remain
the two recorded above and both are UNBUILT.

No tests were added; the brief did not ask for them and the wall clock is not injectable without a
new seam outside this unit. Reference-first check: `.vendor/xfdesktop-master/src/xfdesktop-notify.c`
delegates expiry to libnotify's per-notification `timeout`, so the local single-coalesced-deadline
design is at least equivalent; `.vendor/kwin-master`, `.vendor/smithay-master` and p02's
desktop-widget fix hold no conflicting shell-clock deadline precedent. Reference sources were read
only and never used as a dependency or runtime input.

PARTIAL — the shell clock now aligns its monotonic deadline to the displayed minute (or second when
seconds are shown) by sampling the anchor after the wall-clock read, and it arms no shell deadline
or hidden-panel repaint when the taskbar itself is disabled; `taskbarShowClock` and the always-on
reactor wall clock remain as before. Notification expiry was verified to already use one coalesced
deadline and needed no edit. Two production edits were made, both UNBUILT: the worker contract
forbids builds, tests and runtime checks, so verification is left to the orchestrator. No gate was
loosened, no timer added and no visible pixels changed.
