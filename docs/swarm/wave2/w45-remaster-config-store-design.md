# w45: remastered config store — design

Status: step 1 implemented and UNBUILT; steps 2–6 are planned. No runtime claim is made here;
every statement below is source inspection or counted from the current checkout.

## Current structure (file:line)

Shared primitives already exist:

- `crates/flamewm-api/src/safe_config.rs:16` `MAX_CONFIG_BYTES`, `:19` `KEEP_QUARANTINED`;
  `:42` `read_bounded`, `:52` `decode_read`, `:62` `read_text`, `:78` `corrupt_name`,
  `:94` `quarantine`, `:122` `prune_quarantine`, `:162` `quarantine_and_warn`,
  `:179` `load_or_default`, `:203` `quarantine_before_save`.
- `crates/flamewm-platform/src/storage.rs:9` `write_atomic_text`, `:59` `write_atomic_with`
  (unique sibling temp, sync, rename, sync parent).

Each consumer still wraps those primitives by hand:

- `crates/flamewm-platform/src/settings/load.rs:26` `LoadOutcome`, `:32` `notice_text`,
  `:49` `report`, `:60` `load_settings`; `crates/flamewm-platform/src/settings/persist.rs:15`
  `PersistOutcome`, `:48` `degraded_notice`, `:68` `persist_settings`, `:85` `cleanup_own_temps`.
- `crates/flamewm-explorer/src/prefs.rs:98` `BLAST_RADIUS`, `:108` `load_recent`, `:116`
  `save_recent`, `:123` `load`, `:133` `save`, `:139` `write_atomic` (a second atomic writer).
- `crates/flamewm-terminal/src/store.rs:546` `CONFIG_BLAST_RADIUS`, `:552` `load_config`;
  `crates/flamewm-terminal/src/settings_cache.rs:91` `load`, `:96` `store` (a third atomic
  writer for the first-paint hint).
- `crates/flamewm-platform/src/services/workspaces.rs:19` `count_store_path`, `:24`
  `BLAST_RADIUS`, `:38` `load_persisted_count`, `:46` `persist_count`.

## Problems (measured by counting current source, not by runtime sampling)

1. Four consumer families (settings, explorer preferences, terminal config, workspace count)
   each own a load wrapper and a notice string; three distinct `BLAST_RADIUS` constants exist
   (`load.rs:32`, `prefs.rs:98`, `workspaces.rs:24`, plus `store.rs:546`).
2. At least three separate atomic-write paths exist: `storage.rs:59`, `prefs.rs:139`,
   `settings_cache.rs:96`. They agree on intent but not on a single typed contract.
3. No consumer watches for changes. All four loads are startup/on-demand reads
   (`load.rs:60`, `prefs.rs:108/123`, `store.rs:552`, `workspaces.rs:38`). A repository search for
   `mtime`, `modified(`, `inotify`, `notify` and timer intervals in config consumers found no
   config-change watcher; the only mtime-gated path is the unrelated application catalog
   (`crates/flamewm-applications/src/app_catalog.rs:263`). A file changed by another process is
   therefore invisible until reload/restart.
4. The safe-read and quarantine layer is already shared, but parse, defaults, quarantine
   decision, one-notice policy and write are repeated per consumer, so each can drift.

## Reference evidence (`.vendor/**`, read-only; never a dependency or runtime input)

- `.vendor/kwin-master/src/main_wayland.cpp:207-208` and `options.cpp:88-89`: KWin creates a
  `KConfigWatcher` and connects `configChanged` to a refresh slot. The notification is pushed as
  a signal with group/key names; there is no stat loop.
- `.vendor/xfwm4-master/src/settings.c:1086-1088` and `:1134`: xfwm4 connects the xfconf channel
  `property-changed` signal to `cb_xfwm4_channel_property_changed`, again a pushed callback.
- `.vendor/smithay-master` core has no config-file watcher; change delivery in an event loop is
  done by inserting event sources (`.vendor/smithay-master/smithay-drm-extras/examples/simple.rs:44`).
  This supports a typed watch handle fed by the owner, not a mtime timer.

Chosen precedent: one typed store owns bounded read, parse, quarantine, defaults, atomic write
and at most one notice; it exposes a watch handle. Writers and a native notifier push typed change
events into that handle; consumers drain it from their existing main loop. No polling, no new
dependency: step 1 uses `std` only, and a later native notifier can reuse the already-present
`signal-hook` workspace dependency (currently used by `flamewm-wm`).

## Target design

- `TypedConfigStore`: associated `Value: Default + Clone + Debug` and `Watch: ConfigWatch`;
  `load() -> Loaded<Value>` (value plus at most one `Option<String>` notice),
  `save(&Value) -> Result<(), String>` (atomic write, one notice on failure),
  `watch() -> Watch` (push change handle).
- `ConfigWatch::next_change() -> Option<ConfigChange>` is a non-blocking drain of events already
  pushed by the store or the native notifier. `ConfigChange` distinguishes `Saved` from
  `External` so a consumer can ignore its own write.
- `Loaded<T>` preserves the current startup shape: defaults on a missing or unusable file, one
  notice, no error that can abort startup.
- `MemoryConfigStore<T>` is the deterministic test double; no consumer is migrated in step 1.
- A future `FileConfigStore<T>` will inject parse/serialize/atomic-write closures so it can reuse
  `safe_config` and `storage::write_atomic_text` without taking ownership of product parsers.

## Steps (each compiles on its own)

1. **Implemented here.** Add `flamewm-api::config_store` with the typed trait, the watch trait,
   `Loaded`, `ConfigChange`, and `MemoryConfigStore` plus pure unit tests; export the module from
   `lib.rs`. No consumer changes, no new dependency, no visual change.
2. Add `FileConfigStore<T>` in `flamewm-api` on top of `safe_config` with injected parse and
   atomic writer; tests in temp directories. Still no consumers.
3. Move settings load/persist behind `FileConfigStore` in `flamewm-platform`, preserving
   `LoadOutcome` and `PersistOutcome` text and file format.
4. Migrate explorer preferences, terminal config/theme cache and workspace count to the same
   store; remove the duplicated writers only after their consumers are migrated.
5. Wire change notification: saves push `ConfigChange::Saved`; a single `flamewm-reactor` native
   source pushes `External`; consumers subscribe once and react in the main loop. This step needs
   fresh runtime evidence before any integration claim.
6. Delete leftover per-consumer notice helpers and record any unavoidable path exception in
   `docs/ARCHITECTURE_EXCEPTIONS.toml` as an exact entry.

## Rollback

- Step 1: remove `config_store.rs` and the one `pub mod config_store;` line in `lib.rs`. Nothing
  else references it, so behaviour, file formats and pixels are unchanged.
- Steps 2–6: each migration is one commit; keep the old load/save helpers and notice strings
  until the following step retires them. Reverting a single commit restores the previous path
  without changing stored bytes or the rendered frame. Step 5 can be reverted by unsubscribing
  the consumer while leaving the store intact.
