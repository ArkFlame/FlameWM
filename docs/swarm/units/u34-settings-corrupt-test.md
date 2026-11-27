# u34 — Settings corrupt-file quarantine fault-injection test

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Assertions
below are derived from the current source, not from a fresh test run.

## Ownership — listed before any edit

- `crates/flamewm-platform/src/settings/load.rs` (read only): `load_settings(&Path, u64) -> LoadOutcome`,
  `LoadOutcome { settings, notice }`, `ReadFailure`, `decode_read`, `failure_reason`, `corrupt_name`,
  `notice_text`, private `quarantine`/`report`.
- `crates/flamewm-platform/src/settings/persist.rs` (read only): `persist_settings(&Path,
  &ProductSettings) -> PersistOutcome`, `PersistOutcome::{Durable, Degraded(String)}`, atomic writer
  via `crate::storage::write_atomic_text`.
- `crates/flamewm-platform/src/settings/snapshot_parse.rs`, `snapshot_serialize.rs`, `product.rs`,
  `settings/tests.rs`, `accent_custom_tests.rs`, `storage.rs`, `services/settings_persist.rs`
  (read only): snapshot format, defaults and existing scratch-directory test conventions.
- `crates/flamewm-platform/src/settings/load_tests.rs` (new): four `#[cfg(test)]` unit tests.
- `crates/flamewm-platform/src/settings.rs` (two-line edit): declares `mod load_tests;` under
  `#[cfg(test)]`.
- This log.

No production behavior, manifest or public API changed. Route evidence: `flamewm-source-first`,
`flamewm-parallel-execution`, `flamewm-rust-systems`, `no-adhoc-scripts`. Governance read:
`AGENTS.md`, `scripts/version`, `.opencode/{HARNESS,ROUTES}.md`.

## Test design

The real entry points are called with their exact signatures; the parser is never stubbed. A fixed
epoch (`1_764_200_000`) makes the quarantine name deterministic. Each case gets a uniquely named
scratch directory under `std::env::temp_dir()` that is removed on drop, so tests are parallel-safe
and never touch the real config path.

| Case | Injected state | Assertions |
| --- | --- | --- |
| `corrupt_json_is_quarantined_once_and_defaults_are_used` | A JSON-looking blob written to `settings.conf`; the line-based `key=value` parser rejects its first line. | `settings == ProductSettings::default()`; one notice string, one `flamewm:` marker, no newline, mentions `unparsable`, `using defaults` and the backup path; original path gone; exactly one sibling `settings.corrupt-<epoch>` file holding the exact injected bytes; a second load is silent. |
| `missing_file_loads_defaults_without_quarantine` | No file at all. | Defaults returned, `notice == None`, no file created, scratch directory empty. |
| `old_file_without_new_keys_fills_defaults` | `revision=4` and `accent=#112233` only. | No notice or quarantine; saved keys survive; absent keys (`font_family`, `taskbar_height`, `accent_opacity`, `animation_speed`) equal their current defaults. |
| `persisted_settings_round_trip_through_load` | A non-default snapshot (revision 9, custom accent, taller taskbar, one unknown key) written by `persist_settings`. | `PersistOutcome::Durable`; reload has no notice and equals the snapshot exactly. |

## Verification

UNBUILT by contract. Compilation and `cargo test` are owned by the orchestrator and were not run
here. No build, commit, push or runtime claim is made by this unit.

## Follow-ups

If a future settings key changes the migration rules in `snapshot_parse.rs`, case 3 may need a new
absent-key assertion; cases 1, 2 and 4 are independent of that table. The quarantined file is left in
the scratch directory until drop, so the byte-for-byte preservation check is observable inside the
test only.

PARTIAL — the four fault-injection tests, the `#[cfg(test)]` module declaration and this log are in
place; because this shared-checkout unit forbids builds and test runners, compilation and execution
remain unverified and are delegated to the orchestrator.
