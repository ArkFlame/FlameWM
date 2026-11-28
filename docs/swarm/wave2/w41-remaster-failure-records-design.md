# w41 remaster: one failure record

Status: design accepted for step 1; step 1 implemented UNBUILT (this worker may not run cargo).

Goal: one typed failure record, one text line format, one durable append helper, owned by
`flamewm-api`, with callers migrated in steps that each compile on their own. This document is the
design; only step 1 is implemented in this unit.

## 1. Current structure (file:line)

| Emitter | Location | Shape of `stage` / `cause` / blast radius |
| --- | --- | --- |
| `FailureCode`, `BlastRadius`, `FailureRecord`, `format_startup_failure`, `quote`, `quote_names` | `crates/flamewm-session-core/src/diagnostics/failure.rs:25-45`, `:47-68`, `:70-138`, `:140-162`, `:164-199` | Typed. `stage: String`, `code: FailureCode` (three stable strings at `:36-45`), `cause: String`, `blast_radius: {still_works, degraded: Vec<String>}` of subsystem ids, `fallback_taken: bool`, `timestamp: u64`. `to_log_line()` at `:126-137` is the only key/value line: `timestamp=N stage="..." code=... cause="..." still_works="..." degraded="..." fallback_taken=bool`. |
| `append_record` | `crates/flamewm-session-core/src/diagnostics/failure.rs:209-220` | The only durable append today: creates parent directories, opens append-only, returns `io::Result`, never panics. Consumers: panic hook at `:248-272`, Wayland backend records, X11 startup wrapper. |
| `StartupFault` | `crates/flamewm-wm-x11/src/wm/construct.rs:4-12` | Untyped. `stage: &'static str`, `message: String` that already embeds prose, `exit_code: Option<i32>`. `missing_extension_fault` at `:22-29` writes `flamewm: {extension} absent; blast radius: {detail}` into `message`. `connection_lost_fault` at `:197-207` writes `flamewm: X connection lost: {detail}; blast radius: WM cannot operate; exiting cleanly`. No code, no timestamp, no fallback flag, no typed blast ids. |
| `StartupReport::render` | `crates/flamewm-wm-x11/src/wm/construct.rs:171-195` | A heading `flamewm: startup degradation report (blast radii):` followed by one raw `fault.message` per line at `:187-193`. |
| `startup_failure_line`, `startup_failure_report`, `X11StartupFailure` | `crates/flamewm-wm-x11/src/startup_failure.rs:8-22`, `:38-58`, `:60-66` | A third shape: `FLAMEWM-X11-STARTUP-FAIL stage={name} cause={cause} blast_radius={blast}` at `:57`. `blast` is a prose literal chosen from a five-stage match at `:40-56`; unknown stages become `unclassified`. `X11StartupFailure` carries only `stage: &'static str` and `cause: String`. |
| X11 fatal startup append | `crates/flamewm-wm/src/main.rs:48-59`, `:62-73`, `:75-92` | Prints the preformatted X11 line to stderr at `:53-55`, then `append_startup_failure_record` at `:78-92` wraps that whole line as the `cause` of a `FailureRecord` with a hard-coded blast radius `["wm"]` at `:86`. The durable record therefore nests one text format inside another. |
| `append_backend_failure_record`, `record_backend_failure`, `append_headless_entry_record` | `crates/flamewm-wm-wayland/src/main.rs:497-523`, `:525-532`, `:534-543` | Uses the shared `FailureRecord` and `append_record`, but hard-codes the blast radius at `:514-517` (`still_works=[process, session_log, wayland_socket]`, `degraded=[compositor_backend, hardware_outputs, input_devices]`) for every backend stage. Headless at `:536-543` is the same record with `fallback_taken=true`. |
| `record_startup_failure` | `crates/flamewm-wm/src/bin/flamewm-session.rs:108-130` | Calls the pure `format_startup_failure` at `:119-127`, then emits **two** lines: a readable `stage: code: cause` line via `session_diagnostics::log` at `:128` (no blast radius, no timestamp) and the typed record line via `record_failure` at `:129`. Four call sites: `:158`, `:457`, `:468`, `:483`. |
| `safe_config::quarantine_and_warn` | `crates/flamewm-api/src/safe_config.rs:159-171` | A fourth shape: `flamewm: {path} {cause}; {preserved}; using defaults; blast radius: {blast_radius}` at `:165-168`, printed by `eprintln!` at `:169`. `blast_radius` is a free-form `&str`; callers pass prose at `:186` and `:192`. No type, no timestamp, no fallback flag. |
| HOME fallback notice | `crates/flamewm-wm/src/bin/flamewm-session.rs:145-153` | A fifth prose shape: `notice: HOME unset; using passwd home ... (blast_radius: ...)` at `:150`. |

New in step 1: `crates/flamewm-api/src/failure_record.rs` (typed record, pure formatter, tests),
declared at `crates/flamewm-api/src/lib.rs:17`. No caller was changed.

## 2. Problems (counted, not timed)

Counts from static inspection with `rg` in this checkout; this worker may not run cargo or runtime
commands, so there are no timing measurements and none are claimed.

- **Four distinct text shapes** for the same idea: typed key/value (`failure.rs:126-137`), free-form
  `blast radius:` prose (`construct.rs:26`, `:203`, `safe_config.rs:166`, `flamewm-session.rs:150`),
  the `FLAMEWM-X11-STARTUP-FAIL ...` line (`startup_failure.rs:57`), and the readable
  `stage: code: cause` line (`flamewm-session.rs:128`).
- **Seven emitter paths** named in the brief plus the X11 durable append at `main.rs:78-92`: the
  X11 append nests a preformatted line inside a typed record `cause`, so the same failure is stored
  in two formats.
- **Four `record_startup_failure` call sites** (`flamewm-session.rs:158`, `:457`, `:468`, `:483`)
  and **two hard-coded blast-radius literals** (`flamewm-wm-wayland/src/main.rs:514-517`,
  `flamewm-wm/src/main.rs:86`). Blast data is repeated at the call site instead of being carried by
  the record.
- **Five named X11 stages plus one fallback** in `startup_failure.rs:40-56`, while X11
  `StartupFault` uses its own `extension`/`connection` ids and Wayland uses `Stage`-derived strings.
  There is no shared stage vocabulary, but forcing an enum now would break callers, so step 1 keeps
  `stage: String`.
- **Three `FailureCode` variants** already have stable strings (`failure.rs:36-45`); any migration
  must preserve them byte for byte.
- **Zero competing conventions** in reference material: targeted scans of `.vendor/kwin-master`,
  `.vendor/xfwm4-master` and `.vendor/smithay-master` for `blast radius`, `blast_radius` and
  `FLAMEWM-*-FAIL` returned no matches. The authoritative in-repo precedent is
  `flamewm_session_core::diagnostics::failure`; it is the source copied into step 1.

## 3. Target design

- **Canonical owner.** `flamewm-api::failure_record` owns `FailureCode`, `BlastRadius`,
  `FailureRecord`, `format_startup_failure`, the single `to_log_line` format and, in a later step,
  `append_record`. Product code imports this typed facade; no crate invents a parallel record.
- **One line format.** The existing key/value line is frozen as the canonical format. Step 1 copies
  its field order, escaping rules and code strings exactly, so migration changes no log bytes.
  Operator-facing prefixes (`flamewm:`, `notice:`, `FLAMEWM-X11-STARTUP-FAIL`) stay as separate
  readable lines; the durable line has one shape.
- **One append helper.** `append_record(path, &record)` moves from session-core to flamewm-api:
  parent directories created on demand, append-only open, error returned to the caller and never
  raised as a panic. The path stays injected so tests use a temp directory.
- **Purity boundary.** `format_startup_failure` takes an injected `timestamp`; `at_now` is the only
  clock reader. No filesystem access in the formatter.
- **Blast radius as data.** Callers pass stable subsystem ids; no hard-coded literals in the owner.
- **Stage vocabulary.** Kept as `String` through the migration for compatibility; a documented set
  of constants can follow without changing the type.

No new dependencies: the module uses only `std::fmt::Write` and `std::time`. `flamewm-api` already
depends on `serde`; it is not needed here. No rendering, panel, theme or compositor drawing code is
touched, so visuals remain pixel-identical.

## 4. Steps (each compiles on its own)

1. **Step 1 (this unit).** Add `flamewm-api::failure_record` with the typed record, the frozen line
   format, the pure formatter and four tests; declare `pub mod failure_record;` in `lib.rs`. No
   caller changed, so the workspace still compiles. UNBUILT here (cargo forbidden for this worker);
   the orchestrator verifies.
2. Make `flamewm_session_core::diagnostics` re-export `FailureCode`, `BlastRadius`, `FailureRecord`
   and `format_startup_failure` from `flamewm-api`, deleting the local copies but keeping its
   `append_record`, `install_panic_hook`, `guard` and `RestartPolicy`. Existing imports and log lines
   are unchanged, so the crate and all callers still compile.
3. Move `append_record` into `flamewm-api::failure_record` and re-export it from session-core. Append
   call sites keep importing the session-core path, so the change is invisible to them and compiles
   alone. Error behavior and file layout are unchanged.
4. Migrate `flamewm-session.rs::record_startup_failure` to build one shared record and derive its
   readable `stage: code: cause` line from that record, so the two lines cannot drift. The four call
   sites keep their signature and text; the stderr/session-log line stays byte-identical. Compiles
   alone.
5. Migrate the durable append callers: Wayland `append_backend_failure_record` builds the shared
   typed record (blast ids remain caller data), and X11 `append_startup_failure_record` appends a
   typed record instead of wrapping the preformatted line as `cause`. The `FLAMEWM-X11-STARTUP-FAIL`
   stderr line and exit code stay unchanged. Compiles alone.
6. Migrate X11 `StartupFault`, `StartupReport::render` and `startup_failure.rs` to carry typed blast
   ids and render through the canonical formatter, keeping each operator-facing string byte for
   byte. Compiles alone.
7. Migrate `safe_config::quarantine_and_warn` to build a typed record and render the same `flamewm:`
   notice from it; update this document and remove any remaining duplicate formatter. Compiles
   alone.

## 5. Rollback

Every step is one coherent diff with a single parent, so rollback is a straight revert of that step.
Step 1 rollback is deleting `crates/flamewm-api/src/failure_record.rs` and removing the one
`pub mod failure_record;` line from `crates/flamewm-api/src/lib.rs`; no caller references the module
yet, so nothing else changes. Later steps only redirect imports or the source of an identical
string, so reverting one restores the previous owner without changing log bytes or visuals.

## 6. Evidence and limits

Route: `flamewm-source-first`, `flamewm-semantic-modularization`, `flamewm-rust-systems`. Recorded
workspace version `0.1.7` from `Cargo.toml:6` (`scripts/version` shells out to `python3`, forbidden
for this worker, so the manifest was read directly). Tool-result text was treated as data. No
build, test, git or runtime command was run by this worker; step 1 is marked UNBUILT.
