# p99 settings corruption gaps

STATUS: OPEN

Scope: corrupt, unreadable or oversized config files read at startup. Read-only audit. No crate edits.

## Already covered (good)

- `crates/flamewm-platform/src/settings/load.rs:136` `load_settings`: bounded read (`MAX_SETTINGS_BYTES` 256 KiB, `:17`, `:20-25`), non-UTF-8, unreadable and unparsable input all quarantine to `settings.corrupt-<epoch>[.n].conf` (`:101-122`), fall back to defaults, and emit one `flamewm:` line. Wired from `crates/flamewm-platform/src/host.rs:95`, which also appends a `FailureRecord` (stage `settings`, blast radius session/shell/wm + custom_settings) at `host.rs:100-122`.
- `settings/persist.rs:68`: atomic write, failure degrades to one notice.
- FAULT_TESTS.md line 95 (`settings/load_tests.rs:53, :151, :176, :196`) covers quarantine, truncation, binary junk and huge file for `settings.conf` only.

## Gap table

| # | file:line | Behavior today | Missing fallback | Test needed |
|---|---|---|---|---|
| G1 | `crates/flamewm-platform/src/services/workspaces.rs:23-31` `load_persisted_count` | `read_to_string` with no size cap. Any malformed, out-of-range or unreadable `workspaces.conf` returns `None` silently. No quarantine, no notice, no failure record. The next `persist_count` (`:33-35`) overwrites the bad bytes. | Bounded read; quarantine via `settings::load::quarantine`; one notice plus `FailureRecord`; then default count. | Corrupt, oversized (>256 KiB) and non-UTF-8 `workspaces.conf`: assert quarantine file exists, count is default, one notice, original bytes preserved. Not in FAULT_TESTS. |
| G2 | `crates/flamewm-explorer/src/prefs.rs:98-101` `load`, `:103-107` `load_recent`, `:111-116` `save`, `save_recent` | `read_to_string(...).map(parse).unwrap_or_default()`. A corrupt or oversized `explorer.toml` silently becomes defaults. `save`/`save_recent` then rewrite the file, destroying the bad bytes. No notice. | Same as G1, plus `save` must not overwrite a file that failed to parse until it has been quarantined. | Corrupt `explorer.toml` loads defaults, leaves a `explorer.corrupt-*` copy, and a later `save` keeps the copy. Not in FAULT_TESTS. |
| G3 | `crates/flamewm-terminal/src/main.rs:127-131` | `fs::read_to_string(&config_path).ok().and_then(from_toml).ok().unwrap_or_default()`. Read errors, oversize and TOML errors are all swallowed. No quarantine, no notice, no size cap. | Bounded read, quarantine, one notice (stderr plus session log), defaults. | Corrupt and oversized `terminal.toml` yield defaults, one notice, file preserved. Not in FAULT_TESTS. |
| G4 | `crates/flamewm-platform/src/settings/load.rs:112`, `:100-122` | Quarantine has no retention limit. Each corrupt event adds one file, and the loop tries up to `u16::MAX` names. Old quarantine files are never pruned. | Keep the newest N (for example 5) quarantine files and remove older ones after a successful rename. | Five corrupt loads in a row leave at most N `settings.corrupt-*` files. |
| G5 | `crates/flamewm-platform/src/host.rs:125-128` | After a successful parse, `load_text(...)?` propagates errors. If the serialized snapshot is rejected, `start()` fails. Unverified: I did not confirm that `serialize` of a parsed snapshot can fail validation. | Log and keep defaults instead of aborting `start()`. | Round-trip test: a parsed snapshot that `load_text` rejects must not abort startup. Verify first. |
| G6 | `crates/flamewm-platform/src/settings/load.rs:125-126` | The notice is stderr plus the session log only. The user sees nothing in the shell. This is documented in `host.rs:96-99`, so it is a known limit and not a defect. | Out of scope for this unit. | None. |

## Fix spec (max 3)

### F1 (covers G2, G3): one shared bounded-load helper

1. Make `read_bounded`, `quarantine` and `notice_text` in `settings/load.rs` `pub(crate)` in a shared module. Better: a small `flamewm-platform/src/safe_config.rs` or a new `flamewm-config-safe` crate, so explorer and terminal can use it. Do not duplicate it.
2. Explorer `prefs::load`: on `NotFound` return defaults silently. On any other failure, quarantine, `eprintln!` one line, return defaults.
3. Explorer `prefs::save` and `save_recent`: if `explorer.toml` exists and fails to parse, quarantine it first, then write. Never overwrite unparsable bytes in place.
4. Terminal `main.rs:127`: replace the `.ok()` chain with the same helper. Parse failure: quarantine plus one notice, then `TerminalConfig::default()`.
5. Test (`prefs` tests and terminal config tests): corrupt, oversized and missing cases, as in the G2/G3 rows.

### F2 (covers G1): workspaces count

1. In `services/workspaces.rs:23`, replace `fs::read_to_string(path).ok()?` with the bounded read from F1.
2. Missing file: return `None` silently. Malformed, out-of-range, non-UTF-8, too large: quarantine, one notice, `None`.
3. Test: add the G1 cases to a `workspaces` test module. Assert `load_persisted_count` returns `None`, the quarantine file exists, and the original bytes are intact.

### F3 (covers G4): quarantine retention

1. In `settings/load.rs`, after a successful `fs::rename` in `quarantine` (`:112`), list sibling `<stem>.corrupt-*` files, sort by epoch in name, and delete all but the newest 5.
2. Deletion failures are ignored. Retention must never fail startup.
3. Test: five consecutive corrupt loads leave at most 5 quarantine files.

## Notes

- G5 is unverified and should be checked before any fix.
- No crate edits made. No git, cargo or python run.
- Scope of the fault-injection index (`docs/FAULT_TESTS.md`): settings.conf only. Add rows for workspaces.conf, explorer.toml and terminal.toml when their tests land.

## Result

UNBUILT. No cargo, tests or gates were run; compile and test status is unknown.

- F1 (G2, G3): shared `flamewm_api::safe_config` (bounded read, `ReadFailure`, `quarantine`, `load_or_default`, `quarantine_before_save`) in `crates/flamewm-api/src/safe_config.rs`. Explorer `prefs.rs` and terminal `store.rs::load_config` (called from `main.rs`) use it. Explorer `save`/`save_recent` quarantine before writing. Tests: `prefs.rs` `corrupt_file_tests`, `store.rs` `load_config_tests`.
- F2 (G1): `services/workspaces.rs` `load_persisted_count` uses `load_or_default`; `persist_count` quarantines first. FailureRecord not added (stderr notice only). Tests: `corrupt_count_tests`.
- F3 (G4): `quarantine` prunes `<stem>.corrupt-*` siblings to the newest `KEEP_QUARANTINED` (5). Test: `safe_config::tests::quarantine_keeps_only_newest_copies`.
- `settings/load.rs` now re-exports the shared functions; its existing tests are unchanged.
- Not done: `app/preferences.rs:30` (terminal preferences path) still uses the old `.ok()` chain. G5 unverified and not touched. FAULT_TESTS.md rows not added.
