# Design: y05 startup stage timing

## Current structure (file:line)

- `crates/flamewm-session-core/src/diagnostics/failure.rs:70-80` — `FailureRecord` carries only
  `timestamp: u64` (unix seconds from `unix_seconds_now()` at `:201-207`); no monotonic field.
- `failure.rs:121-137` — `to_log_line()` serializes `timestamp=<u64>` plus five stable fields; no
  elapsed/stage-timing field.
- `failure.rs:140-162` — `format_startup_failure()` injects the wall-clock timestamp only.
- `crates/flamewm-wm/src/bin/flamewm-session.rs:112-130` — `record_startup_failure()` builds a
  record with `unix_seconds_now()`.
- `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:17-47` — `abort_recovery()` takes an
  injected `timestamp`; `flamewm-session/tests.rs:371-408` pins those values.
- `crates/flamewm-wm/src/main.rs:78-90` — `append_startup_failure_record()` uses `FailureRecord::at_now`.
- No stage table exists: search for `stage_table|StageTiming|stage_timing|monotonic` in product
  source finds only an unrelated comment at `flamewm-reactor` (`monotonic fallback`).

## Counted problems

1. Wall-clock only: `timestamp` can move backwards after a clock correction, so two stage failures
   cannot be ordered reliably.
2. No per-stage timing: nothing records how far into startup a stage was reached, only when a
   failure happened.
3. No owner for stage order: stage labels are free-form strings built independently in three
   call sites.
4. No test of a stage table: existing tests pin `timestamp` values but never stage progression.

## Reference precedent (path + choice)

- `.vendor/lightdm-main/src/lightdm.c:105` starts a `GTimer` at log init; `:85` prefixes every log
  line with `g_timer_elapsed()` as `[%+.2fs]`, a monotonic elapsed time. Choice: mirror the
  semantic (injected monotonic elapsed milliseconds, recorded once per stage) inside
  `flamewm-session-core::diagnostics::failure`; do not transplant GLib code and do not add a
  timer dependency.
- `.vendor/sddm-develop` (`src/daemon`, `src/common`) checked: stages are started through `start()`
  slots only; no elapsed-timer or stage-timing precedent found. Corpus is incomplete for SDDM
  timing, so LightDM is the ported reference.

## Target design

- `StageTiming` (stage label + `elapsed_ms: u64`) and `StageTimingTable` in `failure.rs`: one
  monotonic timestamp per stage, first report wins, values clamped non-decreasing, clock injected
  so the module stays pure and testable.
- Add `elapsed_ms: u64` to `FailureRecord` through `FailureRecord::with_elapsed(...)`; keep
  `new()`/`at_now()` writing `0` so existing callers keep compiling, and append
  `elapsed_ms=<u64>` to `to_log_line()` as an additive stable field.
- Wire `flamewm-session` to seed the table at session start from a monotonic `Instant` and feed
  `elapsed_ms` into both `record_startup_failure()` and supervisor abort records.
- Tests: stage-table progression, deduplication and clamping; log-line field; session tests assert
  the monotonic value. Visuals stay pixel-identical (log text only, no rendering path).

## Steps (each compiles alone)

1. Add `StageTiming` + `StageTimingTable` and their unit tests in `diagnostics/failure.rs`, and
   re-export them from `diagnostics.rs`. No call site changes. **IMPLEMENTED, UNBUILT.**
2. Add `elapsed_ms` to `FailureRecord` and `to_log_line()` via `with_elapsed()`; update the two
   log-line assertions in `failure.rs` tests. Compiles without touching callers.
3. Wire session startup: seed the table from `Instant::now()` in `flamewm-session.rs`, pass
   `elapsed_ms` into `format_startup_failure()`/`abort_recovery()`, and update session tests.

## Rollback

- Step 1: remove the added type/test blocks in `failure.rs` and the re-export entries in
  `diagnostics.rs`.
- Steps 2/3: revert the field, serializer and call-site edits; log lines return to their exact
  current text. Appended fields are additive, so no log schema migration is needed.
