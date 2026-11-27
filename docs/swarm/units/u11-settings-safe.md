# u11 — Settings quarantine and safe defaults

Status: **UNBUILT** (edits, including the caller integration below, are complete by inspection only;
the brief forbids `cargo`, builds and tests, so nothing in this unit has been compiled or executed).

## Ownership and route

- Edits: `crates/flamewm-platform/src/settings/load.rs`,
  `crates/flamewm-platform/src/settings/persist.rs`, `crates/flamewm-platform/src/settings.rs`
  (module declarations and `pub(crate)` re-exports), this log.
- File IO only. `product.rs`, `stable.rs` key tables and `storage.rs` are untouched. The callers
  `crates/flamewm-platform/src/host.rs` and
  `crates/flamewm-platform/src/services/settings_persist.rs` were integrated in a second pass and
  are owned by that pass; no other files were changed.
- Route evidence: `flamewm-source-first` (source/callers/owners read first) and
  `flamewm-rust-systems` (ownership, atomicity, failure paths). `AGENTS.md`, `scripts/version`,
  `.opencode/HARNESS.md`, `.opencode/ROUTES.md`, `.agents/{README,CONTRACTS,JOBS}.md` read.
- Evidence consulted: `settings.rs` (`ConfigStore`, `persist`), `host.rs` (`start` read path),
  `storage.rs` (`write_atomic_text`, `quarantine_unparsable`), `services/settings_persist.rs`
  (debounced writer), `snapshot_parse.rs`, `product.rs`, `Cargo.toml` (edition 2024, rust 1.89).

## Audit rows

| Row | Hard fail audited | Before | Safe behavior implemented | Owner path |
| --- | --- | --- | --- | --- |
| U11-01 | Unreadable settings file (permission denied) | `host.rs` returned `FlameError::IoFailure`, aborting startup | `decode_read` maps every non-`NotFound` read error to `Unreadable`; `load_settings` quarantines and returns defaults, never `Err` | `load.rs` |
| U11-02 | Non-UTF-8 settings file | Reported and renamed to `.bak` with two error lines | `decode_read` maps invalid UTF-8 to `NotUtf8`; one `flamewm:` line; renamed to `settings.corrupt-<epoch>` | `load.rs` |
| U11-03 | Unparsable / corrupt settings file | `.bak[.N]` quarantine plus two error lines | `corrupt_name` builds `<stem>.corrupt-<epoch>[.<n>][.<ext>]`; collision-safe rename; defaults load; exactly one notice | `load.rs` |
| U11-04 | Partial write (crash mid-write) | Atomic writer already used a unique temp, fsync, rename, fsync parent | Preserved: `persist_settings` delegates to `write_atomic_text`; `is_own_temp` plus `cleanup_own_temps` recognize leftovers from our pid at startup | `persist.rs` |
| U11-05 | Unwritable config dir / read-only home | `create_dir_all`/open failure propagated and could abort a transaction | `persist_settings` converts any `io::Error` to `PersistOutcome::Degraded` with one notice; in-memory settings stay authoritative | `persist.rs` |
| U11-06 | Concurrent writers | Unique temp names and rename existed, but failures still propagated | Unique sibling temp (pid + sequence) and last-completed-rename-wins remain; failed attempts leave the previous whole file intact and degrade instead of aborting | `persist.rs` |
| U11-07 | Missing settings file | Silent fresh install | `ReadFailure::NotFound` stays silent, defaults returned | `load.rs` |
| U11-08 | Invalid snapshot at save time | Validation error aborted the caller | Validation error becomes a single degraded notice, no abort | `persist.rs` |

## Decisions

- Quarantine name is `<stem>.corrupt-<unix-epoch>[.<n>][.<ext>]` in the config directory. The
  collision suffix prevents a second corrupt file in the same second from overwriting the first.
  Precedent: existing `storage::quarantine_unparsable` preserved data under `.bak`; this unit keeps
  the "never overwrite blind" rule and changes the name to the requested timestamped form.
- A single notification is emitted through the existing `flamewm:` error line (`eprintln!` used by
  `host.rs` and `storage.rs`). `LoadOutcome.notice` and `PersistOutcome::Degraded(String)` carry the
  same text so a caller can assert it; callers must not print it a second time.
- Every load path returns `LoadOutcome` (never `Result`), and every persist path returns
  `PersistOutcome` (never `Result`). A bad file, a read-only home or a failed rename cannot abort
  startup, a settings transaction or shutdown.
- Atomicity is unchanged in mechanism: unique sibling temp, write, `sync_all`, `rename`, `sync_all`
  on the parent directory, with temp removal on failure (`storage::write_atomic_text`).

## Unit tests added (pure parts only)

- `load.rs`: UTF-8 decode, non-UTF-8 rejection, `NotFound` vs unreadable mapping, failure reasons,
  timestamped corrupt name (with and without extension), collision suffix, same-directory
  invariant, invalid path, single-line notices with and without a preserved backup.
- `persist.rs`: writable-target check, temp sibling naming, invalid path, own-temp classification,
  single-line degraded notice, durable/degraded outcome distinction.

## Caller integration (completed)

`settings.rs` declares `mod load;` and `mod persist;` and re-exports `LoadOutcome`, `load_settings`,
`PersistOutcome`, `persist_settings` and `cleanup_own_temps` as `pub(crate)`, so the units are
reachable through `crate::settings::*`.

- `host.rs::start` no longer reads the file itself or uses the two-line `eprintln!` plus
  `storage::quarantine_and_report` `.bak` path. It calls `load_settings(&path, epoch_secs)` and
  adopts the returned `ProductSettings` through the existing `SettingsService::load_text` entry
  point. Quarantine, defaults and the single notice come from `load_settings`; the caller never
  prints `LoadOutcome.notice`. Only a parsed file (revision bumped above the default) is adopted,
  so a missing or quarantined file leaves the default store and revision exactly as before.
- `services/settings_persist.rs::spawn` no longer calls `storage::write_atomic_text` directly in its
  sink. It parses the serialized text into a `ProductSettings` and calls
  `persist_settings(&path, &snapshot)`, mapping `PersistOutcome::Durable` to `Ok(())` and
  `PersistOutcome::Degraded(notice)` to the same `FlameError::IoFailure` the writer already
  returned. The notice is emitted once by `persist_settings`; the caller does not print it. The
  `Sink = FnMut(&str) -> FlameResult<()>` type and all writer methods are unchanged, so tests using
  `with_sink` keep their in-memory sinks.

## Verification

- UNBUILT: no `cargo`, build or test command was run, per the brief. Signatures were checked by
  reading `load.rs`, `persist.rs`, `settings.rs`, `host.rs`, `services/settings.rs`,
  `snapshot_parse.rs` and `snapshot_serialize.rs` before editing. The tests above and the existing
  writer tests are unverified; the orchestrator must compile and run them after integration.
