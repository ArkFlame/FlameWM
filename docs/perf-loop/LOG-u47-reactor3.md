# LOG u47-reactor3

Area: `crates/flamewm-reactor`, `crates/flamewm-dbus-reactor`, `crates/flamewm-control-*`.

Composition reasoning: the control bus is the only per-message D-Bus surface on the WM's own
reactor. Every method call and signal it accepts runs `on_ready` → `name_owner_lost` →
`handle_call`/`decode_bus_signal`, so allocations there are paid per bus message for the whole
session (shell, settings, explorer, task manager clients), not once at startup. The reactor's
`dispatch` runs on every wakeup of every FlameWM process, so per-turn clock reads sit on the
hottest line in the binary's event loop.

## Review of the recent perf commits in these paths

- `next_repeat_deadline` (u11): anchor-based re-arm is correct — the wait is always `<= period`,
  a suspend jumps to the next grid point, and zero-delay repeats keep the old behaviour. No
  regression found.
- `ChildPipe::write_line` (u11): stack staging writes `[..=len]` with `len < 512`, so the newline
  always fits; lines of 512 bytes and above take the two-write path. No regression found.
- `Easing::apply` (u11): polynomial is identical to the previous `powi` curve. No regression.
- One stale comment fixed: `FdReadiness::from` claimed calloop reports terminal/error readiness.
  calloop 0.14's epoll backend hard-codes `error: false` and delivers a peer hangup as plain
  readability, so `hangup` can only come from a backend that reports errors. Comment now says so;
  the mapping is unchanged.

## Changes

1. `flamewm-control-dbus/src/lib.rs` — borrowed path check per bus message.
   `ControlServer::on_ready` compared `message.path().map(|p| p.to_string())` against
   `Some(OBJECT_PATH.to_owned())`: two `String` allocations for every message the WM connection
   sees, including traffic it then drops. Now `message.path().is_some_and(|path| path == OBJECT_PATH)`
   compares libdbus's borrowed `Path` in place. `decode_bus_signal` had the same pattern on every
   client-side signal and is fixed the same way.

2. `flamewm-control-dbus/src/lib.rs` — borrowed interface/member checks.
   `name_owner_lost` formatted both names to `String` just to compare them; it now compares the
   borrowed `Interface`/`Member` values. This runs first for every inbound message.

3. `flamewm-control-dbus/src/lib.rs` — no deep `ControlRequest` clone per call.
   `handle_call` cloned the whole request before dispatch only to test its variant afterwards;
   an `ApplySettings` clone copies the full transaction, `ApplyShortcuts` copies the binding map.
   The variant is now decided up front by `publishes_shortcuts_changed(&request)` plus a
   `matches!` for the weather publish, and the revision read stays lazy behind `Option::then`.

4. `flamewm-control-dbus/src/lib.rs` — sender name only when a lease needs it.
   The calling client's unique name was formatted into a `String` for every call, while only
   `Begin`/`EndHotkeyCapture` consumes it. `sender_name(message)` now runs inside those two
   branches.

5. `flamewm-control-dbus/src/lib.rs` — no signature string per decoded value.
   `from_item` built `item.signature().to_string()` before matching, i.e. one formatted allocation
   per scalar in every snapshot, reply and signal. Arrays now read the element signature from the
   `MessageItemArray` in the guard that needs it (`ay`/`as`); scalar arms allocate nothing extra.

6. `flamewm-reactor/src/lib.rs` — no clock read on turns without a wake deadline.
   `dispatch` called `Instant::now()` on every turn to bound a deadline that is usually `None`.
   The clock is now read only when an owner set a deadline; idle and purely event-driven turns go
   straight to epoll.

## Not changed (with reason)

- `flamewm-dbus-reactor` keeps its `MAX_DISPATCH_PER_TURN` raw-message budget. Raising it or
  draining without a bound would let one flood starve the UI turn, and a self-wake needs reactor
  ownership the pump does not have. Known consequence, unchanged by this unit: if a single
  readiness turn queues more than 32 messages, the remainder waits for the next bus event.
- fd sources stay level-triggered; one-shot re-arm would add an `epoll_ctl` per event.

## Expected composition shift

Per inbound control message: 2 allocations (path) + 2 (signal name checks) removed, plus 1
signature string per decoded value and 1 sender string per non-lease call. Per call that changes
settings/shortcuts: one deep request copy removed. Per reactor dispatch without a deadline: one
clock read removed in every FlameWM process.

## Checks

No `cargo`/`git` in this batch (shared checkout, orchestrator builds). Correctness argued by
reading the exact libdbus/calloop APIs: `Path`/`Interface`/`Member` implement `PartialEq<&str>`,
`MessageItemArray::signature()` returns `&Signature<'static>` (compared by place, no allocation),
and `Option<ControlSignal>` feeds `Vec::extend` with the same ordering as before.
