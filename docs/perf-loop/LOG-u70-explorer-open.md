# u70-explorer-open

Scenario: one second of Explorer cold open (process start through the first painted frame),
classified by pre-paint share. Before this unit the pre-paint sample was dominated by, in order:
(1) the session-bus settings round trip, (2) the synchronous sidebar build in `ExplorerApp::new`
(`user-dirs.dirs` opened six times, a `mountinfo` pass that unescaped every line, `gvfs_root`'s
`/proc/self` stat), (3) template decode, (4) `lsblk`/udev start-up IO, (5) the scan stream. Rows
themselves already stream from the scan worker, so the fix is to stop paying synchronous startup IO
on the paint path and to stop re-doing per-line work.

## Review of the earlier explorer slices (regressions)

Compared the current tree with the recorded intent of `LOG-u01-explorer-io.md` and
`LOG-u02-explorer-ui.md` (no `git log` in this shared checkout). Invariants re-checked by reading:
`lower_cmp`/`contains_lower` still fold ASCII without allocating, `filtered_count` still returns
`entries.len()` for an empty query, `visible_window` still slices instead of filtering, the
interned `IdTables`, `row_labels` and `menu_enabled` caches are present and keyed as documented.
No regression found; the cold-open leftovers were startup-only (none of the steady-state caches).

Changes (all keep visuals and features):

- `main.rs`: the settings fetch (`initial_settings`, up to its 300 ms control-bus timeout) now runs
  on its own thread beside `decode_document(UI)` and `prefs::load`, then is joined before the
  window is built. Pre-paint cost becomes `max(decode, settings)` instead of their sum; the theme
  and backdrop still come from the live snapshot, never from defaults.
- `main.rs`: the listing (`scan`) starts before the template decode again, so rows stream while the
  window is built, and `refresh_devices` (`lsblk` process spawn) plus `watch_devices` (udev/mount
  watch) now start only after the first `project`, moving device IO off the paint path.
- `lib.rs`: `places()` reads `user-dirs.dirs` once and resolves every XDG user directory from that
  text (six opens/parses at startup became one). The user-dir key match is allocation-free
  (`strip_prefix("XDG_")`/`strip_suffix("_DIR")`), replacing a per-line `format!`; the sidebar
  vector is pre-allocated. `user_dir` had no remaining callers and is deleted (dead code).
- `lib.rs`/`add_bookmark`: the XDG config root is computed once by one `config_base` helper.
- `places.rs`: `parse_mountinfo` rejects non-media mount points on the raw field before
  `unescape_mount`, whose four chained `replace` passes allocated a string per mountinfo line
  (hundreds of throwaway allocations per sidebar build).
- `places.rs`: `gvfs_root()` resolves the process uid once (`OnceLock`) instead of stat-ing
  `/proc/self` on every sidebar rebuild (mountinfo, block-device and watch events each trigger one).

Expected composition shift: the settings share leaves the critical path entirely (it was the
single largest pre-paint wall-clock slice whenever the control service answered slowly), sidebar
build loses its file-read and per-line allocation shares, and the device-IO share moves after the
first painted frame. No feature or visual is removed; the sidebar still fills devices, network and
bookmarks from the same sources.

Verification: static review only in this unit (no cargo/git per the brief). Existing tests keep
their contracts: `mountinfo_selects_media_mounts_and_unescapes_names` still gets `/run/media/...`
unescaped before matching, and escaped media names still unescape.

## Review of recent explorer slices (regressions)

Re-read the current tree against `LOG-u01-explorer-io.md` and `LOG-u02-explorer-ui.md` (no `git log`
in this checkout): `contains_lower`/`filter_cache`, `list_directory_streamed_cancellable`,
`fill_sidebar_rows` and the projection id/label caches are still in place and still allocation-free
on their hot paths; no regression found. The one regression found was startup-shaped:
`places()` had drifted back to one `user-dirs.dirs` read per user directory and `parse_mountinfo`
unescaped every mountinfo line before filtering, both restored here.

## Tests

No `cargo`/`git` executed (shared checkout); changes are compile-reviewed by reading.
