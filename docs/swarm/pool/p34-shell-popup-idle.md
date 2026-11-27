# Unit p34-shell-popup-idle
STATUS: DONE

Crate flamewm-shell: popups/menus when closed must hold zero timers and no live documents; find any that keep ticking or retain full documents while hidden and fix.

## Result
STARTED

Audit of `crates/flamewm-shell` popup/menu lifetime using source inspection only (no `git`,
`cargo`, build, test, Python or sleep loop). Reference-first: `.vendor/smithay-master/src/wayland/idle_notify/mod.rs:59-62`
imports `TimeoutAction::Drop` so idle timeouts are one-shot and re-armed only by activity, and
`.vendor/smithay-master/src/wayland/shell/xdg/mod.rs:424-426` records that destroying a popup
dismisses it and releases its surface; the local design already matches this precedent. Reading a
reference never made it a dependency or runtime input.

Every row is **UNBUILT** (static evidence only; the orchestrator builds, tests and checks visuals
after the batch lands).

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/app/timers.rs:235-256` | `publish_wake` merges all shell deadlines into the reactor's one wake slot. Every popup/menu owner is gated: clock `None` while hidden (`:223-225`), OSD/switcher deadlines only while a card is live, `usage_projected_at` filtered by `is_usage_open` (`:253-255`), zone/task-progress/dock-bounce one-shot. No closed popup leaves a wake armed. | NONE — already zero timers while hidden; UNBUILT. | None. |
| `crates/flamewm-shell/src/quick_controls/host/run.rs:163-175`, `:182-192` | The helper loop's only registered timer is the profiler flush, gated on `flamewm_profiler::enabled()`; the weather search source is a wake fd drained only on worker completion, not a timer. No per-popup timer or thread exists (`host/state.rs:3-5`). | NONE — no timer to disarm; UNBUILT. | None. |
| `crates/flamewm-shell/src/event_router/dismiss.rs:44-104`, `notification_center.rs:21-50` | Dismiss/close latches are 300 ms `Instant` timestamps consumed lazily on an opener event; they arm no reactor timer and cause no tick while a popup is closed. | NONE — pure event-time state; UNBUILT. | None; visuals unchanged. |
| `crates/flamewm-shell/src/quick_controls/host/surfaces.rs:10-18`, `:79-115` | `HelperSurfaces` keeps created popup surface handles (and their decoded documents) until shutdown; `close` only ungrabs/unmaps. This is a documented deliberate tradeoff — destroying a live display surface defers native library unloads — not an accidental retention, and it carries no callback or timer. | NONE — retained by design; deliberately not changed. Recreating on close would alter first-frame pixels and undo the prior native-unload gate. UNBUILT. | None from this audit; changing it would risk visual + native-teardown regressions, so left intact. |
| `crates/flamewm-shell/src/quick_controls/host/tick.rs:27-38`, `:97-101` | `Close`/`CloseSession` clear `open` and `active_sequence`; reprojection runs only when `surfaces.open` is `Some`; `weather_search.poll()` is a completion consumer, not a poll source. | NONE — already gated on open state; UNBUILT. | None. |
| `crates/flamewm-shell/src/runtime/lifecycle.rs:171-181`, `app/control.rs:417-451`, `:100` | Closing a context menu unmaps/closes the shared `task_menu` surface, drops `ContextMenuState` (including full tray menu rows) and ends the Start session fields (`start_open`/`start_query`). No full document or model is retained by a hidden menu. | NONE — already released on close; UNBUILT. | None. |
| `crates/flamewm-shell/src/launch_feedback.rs:206-222`, `:490-509` | The launch badge hides and clears poll/animation deadlines and drops the icon worker when the last launch ends; only the intentional silent-launch expiry remains while a launch is in flight. | NONE — already disarms on idle; UNBUILT. | None. |
| `crates/flamewm-shell/src/weather_host.rs:338-380` | `LocationSearch::poll` clears `inflight` on completion and `reset()` drops the completion receiver; no search-owned timer exists. | NONE — completion-driven; UNBUILT. | None. |
| `.vendor/smithay-master/src/wayland/idle_notify/mod.rs:59-62`, `shell/xdg/mod.rs:424-426` (reference only) | Upstream precedent: one-shot, activity-rearmed idle timeouts and popup-destroy dismissal. Confirms the local event/deadline-gated design satisfies the idle requirement. | NONE — reference evidence only; UNBUILT. | None; no runtime dependency added. |
| `docs/swarm/pool/p34-shell-popup-idle.md:1-7` | Unit file had `STATUS: OPEN` with a bare `STARTED` marker. | EDIT (UNBUILT): status `DONE`, marker expanded to this inventory, row table and verdict. Documentation only. | None — docs only; no code path touched. |

## Verification

No compiler, test runner, `git`, Python or sleep loop was invoked, so no runtime measurement backs
this result; every entry is static source evidence marked UNBUILT. No production file was changed,
therefore no gate was loosened, no timer was added and no pixel can move. The single edit is this
unit file.

PASS — the audit covers every popup/menu path in `crates/flamewm-shell`: the shell publishes one
shared reactor deadline with every owner gated on visible/live state, the quick-control helper owns
only a profiler-gated timer plus completion fds, dismiss latches are lazy timestamps with no tick,
and closing a menu drops its state and the shared popup surface is reused rather than duplicated.
The only document retention after close is the helper's deliberately cached surface handles, whose
comment records the native-unload rationale; recreating them would change first-frame pixels and
undo a prior gate, so it was left intact rather than "fixed" at the cost of behavior. Closed
popups/menus therefore hold zero timers and no accidental live documents, visuals stay
pixel-identical, and with no production change there is nothing for the orchestrator's build to
break, though the static-only evidence and UNBUILT rows remain as stated.
