# x05 settings atomic store — design

## Question and owner

Question: how does the settings store get crash-safe atomic writes with an explicit fsync
policy, and one reload per burst of external changes?

Active owners: `flamewm_platform::settings` and its typed children `settings::persist`,
`services::settings_persist`, `storage`; and the shared read/quarantine facade
`flamewm_api::safe_config` (exported at `crates/flamewm-api/src/lib.rs:30`). UI, WM authority,
coordinates and grabs are not involved; visuals stay pixel-identical. No new dependencies.

## Current structure (baseline, before step 1)

- `crates/flamewm-api/src/safe_config.rs:1-6` — module contract is explicitly bounded reads and
  quarantine "for small `FlameWM` config files". It exposes `read_bounded` (`:40-47`),
  `decode_read` (`:50-57`), `read_text` (`:60-62`), `quarantine` (`:92-116`),
  `load_or_default` (`:177-196`) and `quarantine_before_save` (`:201-207`). Write helpers: 0.
- `crates/flamewm-platform/src/storage.rs:7-38` — `write_atomic_text`: static collision counter,
  unique sibling temp, `create_new`, mode `0600`, `write_all`, `sync_all`, rename, then best-effort
  parent-directory `sync_all`; retries 16 times on `AlreadyExists`.
- `crates/flamewm-platform/src/storage.rs:59-88` — `write_atomic_with`: the injectable core that
  performs the temp write, file fsync, rename, temp cleanup on failure and parent fsync after commit.
- `crates/flamewm-platform/src/settings/persist.rs:62-79` — `persist_settings` validates the
  snapshot, rejects an empty path, delegates to `storage::write_atomic_text`, and degrades to an
  in-memory notice on any error.
- `crates/flamewm-platform/src/services/settings_persist.rs:18-21` — `SETTINGS_IDLE_DEBOUNCE`
  (300 ms) and `SETTINGS_MAX_DEFER` (2 s).
- `crates/flamewm-platform/src/services/settings_persist.rs:140-209` — `run` coalesces a burst of
  `Schedule` commands into one trailing-edge write of the newest text, bounded by the max defer;
  `WriteNow` supersedes pending text and `Flush` reports delayed failures.
- `crates/flamewm-platform/src/services/settings_persist.rs:50-71` — the production sink parses and
  clamps the text, then calls `settings::persist_settings`.
- Other `safe_config` consumers: `crates/flamewm-platform/src/services/workspaces.rs:6`,
  `crates/flamewm-explorer/src/prefs.rs:7`, `crates/flamewm-terminal/src/store.rs:553` — all read
  through the shared facade, none can write atomically through it.

### Counted problems

- 1 shared config facade (`flamewm_api::safe_config`) with 0 write primitives; 3 consumers use it
  for reads and quarantine only.
- 2 owners encode the same write-temp-rename-plus-fsync policy: `flamewm_platform::storage` has a
  private writer, while `flamewm_api::safe_config` — the facade those consumers already depend on —
  has none.
- 0 reload debounce: a repository search over `crates/` for `reload`, `watch`, `inotify`, `notify`
  and `note_change` finds only the write-side `SettingsWriter` and an unrelated
  `presentation_scale::notify_scale_change`; no per-path reload coalescer exists.
- 1 write-side debounce already satisfies "one write per burst", so the missing half of the goal is
  explicitly the reload side and the shared write primitive.

## Reference evidence and choice

- `.vendor/kensaku-master/src-tauri/src/config.rs:493-526` — `save_to` creates the parent directory,
  writes `<dir>/config.json.tmp`, `write_all`, `sync_all`, `fs::rename` over the real file, then a
  best-effort `File::open(dir).sync_all()` guarded by `cfg(unix)`, with the comment that some
  filesystems reject directory fsync. This is the direct write-temp-rename and fsync-policy precedent.
- `.vendor/kensaku-master/src-tauri/src/indexer/watcher.rs:47-100` and `:110-143` — `PendingUpserts`
  coalesces create/modify events per path, resets a `SETTLE` (400 ms) deadline on each event,
  exposes `next_deadline`, and re-queues fresh files until the `(size, mtime)` fingerprint stops
  moving. This is the reload-debounce precedent for steps 3-4.
- `.vendor/mate-desktop-master/libmate-desktop/mate-rr-config.c:1262-1278` — renames the intended
  file to a backup, calls `g_file_set_contents`, and rolls the backup back on failure.
  `.vendor/mate-desktop-master/libmate-desktop/mate-desktop-thumbnail.c:1191-1192` — `g_chmod` the
  temp to `0600` then `g_rename` it over the target. Secondary precedent confirming sibling temp,
  private permissions and atomic rename.

Choice: port kensaku's `save_to` fsync policy into `flamewm_api::safe_config` as a typed
`atomic_write_text` helper, keeping the collision-safe `create_new`, `0600` mode and retry loop
already proven by `flamewm_platform::storage` (which matches mate's chmod-then-rename). Use the
kensaku watcher coalescer as the reference for the later reload step, not as code to transplant.
No vendor code, layout or dependency is copied; `.vendor/**` stays read-only reference evidence.

## Target design

The store stays behind the two typed facades; each step compiles alone.

1. **(implemented, step 1)** Add `safe_config::atomic_write_text` as the canonical write-temp-rename
   plus fsync policy in the shared facade, with unit tests. No existing caller changes.
2. Make `flamewm_platform::storage::write_atomic_text` delegate to the new facade helper and delete
   its duplicate temp/loop implementation, keeping `write_atomic_with` for failure-injection tests.
   The public signature is unchanged, so this compiles alone.
3. Add a typed per-path `ReloadCoalescer` behind `flamewm_platform::settings`, ported from the
   kensaku watcher semantics (`note` / `due` / `next_deadline`, quiet window, fresh-file
   fingerprint re-check), so a burst of external changes triggers one reload. Additive, compiles alone.
4. Wire the coalescer to the settings reload entry point and fence self-writes from the writer
   thread so an atomic rename cannot trigger a reload loop. Compiles alone once step 3 exists.
5. Record the fsync policy in the module docs. Documentation only.

Steps 2-5 are out of scope for this unit.

## Step 1 (implemented, UNBUILT)

- `crates/flamewm-api/src/safe_config.rs:8-11` — imports extended with `Write`,
  `std::os::unix::fs::OpenOptionsExt` and `std::sync::atomic` for the writer.
- `crates/flamewm-api/src/safe_config.rs` — new private `NEXT_TEMP_ID` counter and public
  `atomic_write_text(path, text)`: rejects a path without a file name, creates the parent directory,
  and for up to 16 attempts opens a unique sibling temp with `create_new` and mode `0600`, writes
  all bytes, `sync_all`s the file, renames it over `path`, removes the temp on any failure, and
  finally performs a best-effort parent-directory `sync_all`. `AlreadyExists` selects a fresh temp.
- `crates/flamewm-api/src/safe_config.rs` — two tests: whole-file replacement restores `0600` and
  leaves no temp behind, and a path without a file name is rejected.

This step compiles alone: it uses only existing `std` facilities plus the module's own imports, adds
no dependency, changes no existing signature, callsite, on-disk format or visual output.

## Rollback

Delete the two added imports, the `NEXT_TEMP_ID` static, `atomic_write_text` and its two tests.
No existing function was modified, no caller was rewired and no file format or path convention
changed, so there is nothing else to restore and existing config files remain readable.

## Verification status

No build or test was run in this unit (worker rules forbid `cargo` and builds). Step 1 is UNBUILT;
the coordinator runs the build and the scoped `flamewm-api` tests after the batch lands.
