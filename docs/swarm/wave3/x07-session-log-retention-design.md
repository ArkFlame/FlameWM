# x07 session log retention — design

## Question and owner

Question: how does the session bound its persistent failure log and keep a bounded backup?
Active owner: `flamewm_session_core::diagnostics` and its typed child facade
`diagnostics::failure`. No UI, WM authority, coordinate, or grab behavior is involved; visuals
are unchanged. No new dependencies.

## Current structure (baseline, before step 1)

- `crates/flamewm-session-core/src/diagnostics.rs:281-297` — `session_log_path`: resolves
  `$XDG_STATE_HOME/flamewm/session.log`, then `$HOME/.local/state/flamewm/session.log`, then a
  `/tmp` fallback. Pure path resolution, no I/O.
- `crates/flamewm-session-core/src/diagnostics.rs:299-303` — `fallback_log_path`: per-uid `/tmp`
  fallback.
- `crates/flamewm-session-core/src/diagnostics.rs:306` — `LOG_ROTATE_BYTES: u64 = 1 << 20`: the
  size cap, declared but unreferenced by production code.
- `crates/flamewm-session-core/src/diagnostics.rs:309-313` — `rotated_log_path`: appends `.1`;
  only caller was the unit test at `diagnostics.rs:586-591`.
- `crates/flamewm-session-core/src/diagnostics/failure.rs:209-220` — `append_record`: creates
  parent dirs and appends one `key=value` line; documented as "never rotates or truncates".
- `crates/flamewm-session-core/src/diagnostics/failure.rs:255-272` — `install_panic_hook`: the
  only production caller of `append_record` (baseline `failure.rs:268`), with errors discarded.
- `crates/flamewm-session-core/src/diagnostics.rs:15-19` — `append_record` is re-exported through
  the `diagnostics` facade, so callers outside the crate can use it directly.

### Counted problems

- 1 size-cap constant with 0 production readers.
- 1 rotation-path helper with 1 test-only caller and 0 production callers.
- 1 append writer with 0 rotation: a panic/crash loop grows `session.log` without bound.
- 2 declared retention facilities dead, 0 enforced retention behaviors.

## Reference evidence and choice

- `.vendor/lightdm-main/src/log-file.c:18-49` plus `.vendor/lightdm-main/src/log-file.h:17-24`:
  `log_file_open` with `LOG_MODE_BACKUP_AND_TRUNCATE` renames `<name>` to `<name>.old`, then opens
  `<name>` fresh with `O_WRONLY | O_CREAT | O_TRUNC` and mode `0600`; the alternative
  `LOG_MODE_APPEND` just appends. The rename supplies one complete previous generation and a fresh
  current file.
- `.vendor/sddm-develop/src/common/MessageHandler.h:72-120`: append-only `QFile`, falling back to
  truncate when append open fails. No size cap and no rotation, so it does not satisfy the goal.

Choice: port LightDM's backup-and-truncate semantics, not SDDM's append-only behavior. Adapted to
project types: keep the existing `LOG_ROTATE_BYTES` and `rotated_log_path` as the policy knobs,
remove a stale `.1` first (Rust `fs::rename` overwrites on Unix but not everywhere), rename
`<name>` to `<name>.1`, then append to a newly created `<name>`. No vendor code, layout, or
dependency is transplanted; `.vendor/**` stays read-only reference.

## Target design

Retention stays entirely behind the `diagnostics` facade:

1. Enforce the cap at every write (step 1, implemented below): `append_record` rotates before
   appending via a private typed helper in `failure.rs`.
2. Add a typed `LogRetention` config (cap bytes, generation count) in `diagnostics.rs` and make the
   cap a field, replacing the raw constant for callers that need a non-default policy.
3. Wire session startup to rotate once at launch for LightDM parity; append-time rotation remains
   the backstop for crash loops within one session.
4. Generalise to `.1..N` with a generation cap and a count-based test. Steps 2-4 each compile alone
   and are out of scope for this unit.

## Step 1 (implemented, UNBUILT)

- `crates/flamewm-session-core/src/diagnostics/failure.rs:214-227` — `append_record` calls
  `rotate_if_oversized(path)?` after parent creation and before opening for append; doc comment
  now states the rotation behavior.
- `crates/flamewm-session-core/src/diagnostics/failure.rs:229-241` — new private
  `rotate_if_oversized`: no-op when `metadata` length is below `LOG_ROTATE_BYTES` (including a
  missing file), otherwise removes a stale `.1` (tolerating `NotFound`), renames `<name>` to `.1`,
  and returns other I/O errors to the caller.
- `crates/flamewm-session-core/src/diagnostics/failure.rs:528-559` — one test seeds a log at
  exactly the cap with a stale `.1`, appends a record, and asserts the `.1` holds the old bytes
  while the fresh log holds only the new record line.

This step compiles alone: it uses only `std::fs`, `std::io`, and the two existing items in the
parent `diagnostics` module. No signature changes, no new deps, no visual change.

## Rollback

Revert the three regions above: delete `rotate_if_oversized`, remove the `rotate_if_oversized(path)?;`
line and restore the original `append_record` doc comment, and delete the new test. No on-disk
format or path convention changes, so existing logs remain readable and no data migration is
needed. A rotated `.1` file left by this step is harmless stale data and may be deleted.

## Verification status

No build or test was run in this unit (worker rules forbid `cargo`/builds). Step 1 is UNBUILT;
the coordinator runs the build and the scoped test after the batch lands.
