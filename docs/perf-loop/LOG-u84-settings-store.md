# u84 settings store log

Scout of the Settings client area owned by this unit: `crates/flamewm-settings` and
`crates/flamewm-settings-core`. The authoritative store (`flamewm-platform/src/settings.rs`)
and its debounced writer are outside the owned paths and already single-owner, so the
remaining work in this area is on the read side of the Settings window.

Early findings, all pre-existing: bus signals already reproject one stale section and drop
echoes at the projected revision (`control.rs::reproject_scope`), writes on a live slider
drag are already coalesced (`SLIDER_WRITE_INTERVAL_MS`), and the window loop is
reactor-driven with no timer (`runtime.rs`). The polling left in the area is the one-shot
GPU probe child loop, which only runs after an explicit Check press.

Remaining dominant share of a Settings second with the Startup or Default Applications page
open: the page resyncs on every action and key event, and each resync read files:
`startup_entries()` walked every XDG autostart directory and read every entry, then
`startup_candidates()` repeated that walk and additionally scanned and parsed every
installed `.desktop` file across every data dir for the picker. On the Defaults page,
`handler_index()` did the same full catalog read and parse for seven MIME rows. Composition
of that repaint was almost entirely file reads and desktop-entry parsing; the visible
difference between two repaints is normally nothing.

Changes (owned paths only, no feature or visual change):

- `flamewm-settings-core/xdg_applications`: new `ApplicationSettings::stamp()` returning an
  `XdgStamp`. A stamp is one `metadata()` per watched path (each config dir's autostart
  directory, each `mimeapps.list` name, each data dir's applications directory) and reads no
  contents. Steering writes go through `atomic_write`, which replaces a directory entry, so
  a write always moves the stamp it touches; a read taken before a concurrent write leaves
  a stale stamp and forces the next repaint to reload. Composition shift: a repaint of an
  unchanged page drops from "read and parse every desktop entry" to a handful of `stat`
  calls.
- `flamewm-settings-core/xdg_startup`: `startup_candidates_with(&entries)` accepts entries
  the caller already read; `startup_candidates` delegates to it. One repaint listed the
  autostart directory twice (once for rows, once inside the candidate projection); that
  duplicate walk is gone.
- `flamewm-settings/xdg_cache`: new `XdgCache` holding the last Startup view (rows plus
  candidates) and the last Defaults labels with the stamp they were read under. A repaint
  returns the cached `Rc` when the stamp is equal and reloads once when it moved. Borrowing
  is a short `RefCell` and the returned `Rc` is cloned before the borrow ends. The cache is
  cleared on `refresh()` (page entry) and after every write path (`StartupToggle`,
  `StartupRemove`, `StartupDelay`, `StartupAddPicked`, a submitted custom command,
  `DefaultCycle`), so a user change is never masked by the memo.
- `flamewm-settings/default_apps_page`: `labels_with(&ApplicationSettings)` splits the
  environment-independent body out of `labels()` so the Defaults page can reuse one facade
  and the cache; `labels()` keeps its old behavior for any other caller.
- `flamewm-settings`: `SettingsApplication::xdg_cache` field plus wiring in `sync_startup`
  and `sync_defaults`.

Steady-state composition after the change: a Settings repaint on either XDG page is
document writes plus six `stat` calls on a typical one-config/one-data-dir host; file reads
and desktop-entry parsing appear only on page entry, on a stamp move, or once after a write.

Tests: `stamp_is_stable_until_a_watched_path_changes` in `xdg_applications` pins the
contract the cache depends on (stable across reads, moves on an added or removed entry).
Existing page tests in `xdg_applications`, `xdg_startup`, `startup_page` and
`default_apps_page` keep their behavior since they build `ApplicationSettings` directly and
do not go through the cache. Builds and test runs are done by the orchestrator; no cargo was
run in this unit.
