# Log u17-settings-assistant-lock

Scouted: `flamewm-settings`, `flamewm-settings-core`, `flamewm-assistant`,
`flamewm-assistant-core`, `flamewm-lock`, `flamewm-ribbons`, `flamewm-skin`.
The lock loop and the Ribbons rasterizer already gate repaint on a paint key, park the
frame pacer on a condvar, reuse scratch buffers and cap caches; the assistant chat store
has a bounded B-tree cache and streaming-state ticker. The remaining composition share in
this area is the Settings app's **live XDG work on every refresh**, which runs on the UI
thread and is repeated per row.

- `crates/flamewm-settings/src/file_manager.rs` (`current`): drop the `xdg-mime` child
  process that was spawned on every `refresh`. Defaults are resolved through the shared
  `ApplicationSettings` facade, which reads the same `mimeapps.list` chain with the same
  precedence. Expected: one fork+exec per refresh removed (process spawn was the largest
  single cost of a Settings refresh).
- `crates/flamewm-settings-core/src/xdg_applications.rs` (`handler_index`, `default_map`,
  `installed_handlers`, `mimeapps_names`): one directory pass now builds a
  MIME -> handlers index with the previous per-ID masking rules, and one read pass now
  resolves the defaults for a whole page. `installed_handlers` keeps its contract as a
  single-type lookup over that index. Expected: the Default Applications page goes from
  7 scans of every `applications/` entry plus 7 `mimeapps.list` walks to one of each.
- `crates/flamewm-settings/src/default_apps_page.rs` (`labels`, dead `current_label`
  removed) and `app_impl_3.rs` (`sync_defaults`): project all seven rows from the batched
  index and default map instead of re-reading the environment per row.
- `crates/flamewm-settings/src/app_impl_2.rs` (`sync_nodes`): the Startup and Defaults
  row syncs run only while their page is the visible one; switching pages already runs a
  refresh, so a shown page is still current. Expected: no autostart directory read and no
  application catalog scan while the user is on any other Settings page.
- `crates/flamewm-settings-core/src/xdg_startup.rs` (`startup_candidates`): fold the
  case-insensitive sort key once per candidate instead of twice per comparison, and use a
  set for the autostart-ID exclusion instead of a linear scan per entry. Expected: fewer
  short allocations and O(n log n) cheaper for large application catalogs.

All changes keep the same visuals, labels, precedence rules and error text paths.
