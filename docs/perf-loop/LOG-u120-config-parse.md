# u120 config parse log

Scope: `crates/flamewm-settings-core` and `crates/flamewm-settings` (the `flamewm-skin`
crate is compile-time constants and has no runtime parse). `flamewm-config*` does not exist
in this repository.

Scout of what still parses on the config path after `u84` (stamp memoization of the XDG file
reads) and `u107` (settings fetch off the first-frame path):

- The Startup and Default Applications pages re-project on every input event. Each
  projection called `ApplicationSettings::from_environment()` again: `HOME`, `XDG_CONFIG_HOME`,
  `XDG_CONFIG_DIRS`, `XDG_DATA_HOME`, `XDG_DATA_DIRS` and `XDG_CURRENT_DESKTOP` lookups, a
  `split_paths` allocation per directory variable, and a lowercased `XDG_CURRENT_DESKTOP`
  list. That work was repeated even when the projection was a cache hit, because the facade
  had to exist before the stamp could be taken.
- `ApplicationSettings::stamp()` rebuilt the watched-path list on every call: a `Vec<String>`
  of `mimeapps.list` names plus one `root.join(...)` `PathBuf` allocation per watched path,
  then `StampEntry::of(path: PathBuf)` cloned each of those paths before statting it. So an
  unchanged repaint paid two allocations per watched path plus the env parse above, before
  the cheap `stat` loop even started.
- Theme parse at Settings startup is already off the UI thread (`start_theme_scan` spawns
  `flamewm-settings-theme-scan`) and its result is handed over by channel; the theme registry
  is cached until a rescan, and `selected_preview` only borrows the cached `Arc<Vec<u8>>`.
  Nothing on the UI thread re-parses theme files.

Changes (owned paths only, no feature or visual change):

- `flamewm-settings-core/xdg_applications`: new `XdgWatch` (prebuilt watched-path list) and
  `ApplicationSettings::watch()` / `stamp_with(&XdgWatch)`. `stamp()` is now
  `stamp_with(&self.watch())`, so existing callers keep identical semantics while a caller
  that stamps repeatedly can build the path list once. `StampEntry::of` takes `&Path`
  instead of an owned `PathBuf`, removing the per-path clone in the stat loop. Watch order is
  unchanged, so stamps stay comparable with the ones observed before this change.
- `flamewm-settings/xdg_cache`: `XdgCache` now owns the environment facade and its
  `XdgWatch`, built on first use (`prepare`) and reused for the life of the Settings process.
  `startup()` and `defaults()` no longer take a facade parameter; they stamp with the cached
  watch list and keep the existing per-page `XdgStamp` memoization and `Rc` sharing. The
  env-unavailable fallbacks (`StartupView::default()`, seven "Unavailable" labels) moved
  into the cache so both call sites have one owner. `clear()` still drops only the
  projections, never the environment facade.
- `flamewm-settings/app_impl_3`: `sync_startup` / `sync_defaults` drop their per-repaint
  `ApplicationSettings::from_environment()` call and use the cache directly.

Expected composition shift: on the Startup and Default Applications pages, a repaint that
hits the stamp cache now costs only the `stat` calls; the environment parse, path joins,
`mimeapps` name strings and path clones are once per process instead of once per settle.
Startup of the Settings process is unchanged (the facade is also built lazily on the first
projection, not before). No files are read more often than before, and the write paths keep
clearing the projection cache, so a user change still shows on the next repaint.

Test added: `reused_watch_stamps_like_a_fresh_walk` in `xdg_applications` asserts
`stamp_with(&watch)` equals `stamp()` before and after the watched directories appear, and
that the cached watch list is not empty.

Verification: no cargo and no git in this unit (shared checkout). `rustfmt --edition 2024
--check` passes on every edited file, so all edits parse. Compile and tests run in the shared
gate after the batch.
