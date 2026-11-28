# z09 portal / D-Bus absent paths: typed retry design

STATUS: DESIGN DELIVERED; step 1 implemented UNBUILT.

Scope: `crates/flamewm-integrations-linux` only. No new dependency, no visual change, no change to
WM authority or to the typed snapshot contracts. `.vendor/**` stays read-only evidence.

## Current structure (file:line, before step 1)

- `crates/flamewm-integrations-linux/src/lib.rs:20-46` — provider modules are public; `:48-62` is the
  typed facade re-export block; `:95-108` `LinuxIntegrationRuntime::connect()` calls each provider
  `connect()` with `?`, so a hard connect error still aborts startup.
- `crates/flamewm-integrations-linux/src/portal_file_chooser.rs:18-31` — portal service/path/member
  constants and `PORTAL_ABSENT_MESSAGE`; `:34-36` one-shot `PORTAL_ABSENT_RECORDED` guard and
  `PORTAL_ABSENT_EVENT`; `:39-45` `portal_absent()` emits at most one rate-limited line and returns
  `FlameError::unavailable`; `:48-51` `is_portal_absent()` classifies only `ServiceUnknown` for the
  portal name.
- `portal_file_chooser.rs:106-148` — `open_request()` makes exactly one `send_with_reply` call;
  `:137-141` maps an absent service straight to the terminal error with no second attempt.
- `portal_file_chooser.rs:465-487` — one fault test covers classification, message wording and
  "other errors keep their cause", but not a backoff budget.
- `crates/flamewm-integrations-linux/src/network_slot.rs:16-50` and `power.rs:169-188` — system-bus
  providers already degrade to `Unavailable` snapshots; `network_slot.rs:180-205` uses a child
  process with a bogus `DBUS_SYSTEM_BUS_ADDRESS` as an absent-bus fault test.
- `crates/flamewm-integrations-linux/src/power_profiles.rs:158-170`, `bluetooth.rs:505`,
  `backlight.rs:126` — connect with `BusPump::connect(...).ok()` and keep a `None` pump.
- `crates/flamewm-integrations-linux/src/mpris.rs:170`, `launcher_entry.rs:55`, `job_view.rs:64`,
  `polkit_agent.rs:612-613` — connect with `?`, so a missing bus surfaces as a hard error.
- `crates/flamewm-debug/src/lib.rs:210-239` — `emit(id, cooldown, message)` is the rate-limited
  record primitive; `crates/flamewm-api/src/system.rs:19-25` — `ServiceAvailability` is the typed
  snapshot vocabulary.

## Counted problems

1. `portal_file_chooser.rs:137-141` treats a transiently absent portal as terminal; a portal that is
   activated moments later is never retried.
2. No typed retry/backoff owner exists in the crate, so any future service retry would duplicate the
   policy and its sleep boundary.
3. Absence handling is inconsistent: `network_slot.rs:38-49` and `power.rs:172-180` degrade, while
   `mpris.rs:170`, `launcher_entry.rs:55`, `job_view.rs:64` and `polkit_agent.rs:612` propagate `?`.
4. Only the portal has a structured one-shot record (`portal_file_chooser.rs:39-45`); the other
   absent services set `ServiceAvailability::Unavailable` with no diagnostic line.
5. `portal_file_chooser.rs:465-487` never asserts that a retry budget was spent before degrading.

## Target design (typed facade remastering)

`flamewm-integrations-linux` becomes the single owner of "absent service" policy. A new
`absent_service` module holds `AbsentRetry`, a pure bounded-exponential state machine: `next_wait()`
returns the next delay or `None` when the budget is spent, and never performs the wait itself. The
provider keeps its own pump/worker boundary, so no second runtime or blocking primitive is added.
The module is re-exported through the existing facade block in `lib.rs`; product code imports the
facade, not the private module layout.

Step 1 applies `AbsentRetry::PORTAL` to `open_request()`: on a classified absence it records once via
the existing `portal_absent()` guard, waits through `portal_absent_step()`, and issues a fresh
`OpenFile` call; after the budget is exhausted it returns the same clear `PORTAL_ABSENT_MESSAGE`.
Options are built once and cloned per attempt (`MessageItem` is `Clone`); the handle token and
expected request path are unchanged, so D-Bus ownership and response matching are untouched. Pixels
and every typed snapshot stay identical — only failure-path timing changes.

Later steps fold the one-shot record into the same typed owner (`AbsentRecord` keyed by service name)
and spread the policy to the remaining providers, one fault test each, without changing the facade
signatures existing consumers use.

## Steps (each compiles alone)

1. **Implemented (step 1, UNBUILT).** Add `absent_service.rs` with `AbsentRetry` and two pure tests;
   declare and re-export it in `lib.rs`; consume `AbsentRetry::PORTAL` in `open_request()` through a
   private `portal_absent_step()` seam and add one portal fault test. Private typed items, existing
   imports only — compiles alone.
2. Move the one-shot record into `absent_service.rs` as `AbsentRecord` keyed by a `&'static str`
   service name; portal calls the facade. No behavior or call-site change outside the portal.
3. Apply `AbsentRetry` + `AbsentRecord` to the already-degrading system-bus providers
   (`power.rs`, `power_profiles.rs`, `backlight.rs`, `bluetooth.rs`), one fault test each.
4. Convert the `?` connect sites (`mpris.rs:170`, `launcher_entry.rs:55`, `job_view.rs:64`,
   `polkit_agent.rs:612`) to the same typed degrade path with one fault test each.
5. Surface per-service retry counts through the existing `ServiceAvailability` snapshot facade; no
   pixel path, WM authority, or public signature change.

## Ported reference (path + choice)

- Paths: `.vendor/cosmic-epoch/cosmic-session/src/main.rs:142-144`
  (`RestartMode::ExponentialBackoff(Duration::from_millis(10))`);
  `.vendor/cosmic-epoch/cosmic-player/src/xdg_portals.rs:38-44`
  (`inspect_err(|e| warn!(...))` on a failed portal call, then continue the loop);
  `.vendor/mate-desktop-master/xdg-portal/mate-portals.conf:1-3` (portal backend selection only —
  no in-process retry precedent in that corpus).
- Choice: port only the *semantic shape* — a base delay that doubles up to a cap, a bounded retry
  budget, and one warning record per failed attempt — into the pure Rust `AbsentRetry`. Do not
  import `launch_pad`, `ashpd`, or any upstream code; reference material is never a dependency or
  runtime input.

## Rollback

Revert `portal_file_chooser.rs::open_request()` to its single-call form and drop
`portal_absent_step()` plus the new fault test; remove the `absent_service` `pub mod`/`pub use` lines
from `lib.rs`; delete `absent_service.rs`. No manifest, no other provider and no facade consumer is
touched by step 1, so rollback restores the prior behavior exactly. Later steps revert
independently in reverse order.

## Verification limit

UNBUILT by handoff: no `cargo`, test run, or runtime observation was performed. Static inspection
only; the fault tests are written but unexecuted.
