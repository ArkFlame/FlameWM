# u43-settings-lazy

Owned paths: `crates/flamewm-settings*/src/**` (git log unavailable in the shared checkout: `git` is
forbidden for this worker, so the recent-commit review below reads the current code and the
guards the existing tests encode instead of diffs).

Composition read: Settings is not an idle-cost area. The window has no timer and no frame tick;
`runtime.rs` wakes it only from the reactor (`SettingsSignals`, `WallpaperChooserWake`) and from
input, and `reproject_scope` already refreshes one authoritative section per bus signal. Its cost
therefore sits entirely in the *event path*: every key event, pointer motion and wake re-projects
the whole document. The changes below cut that per-event projection, not idle work.

## Changes

- `app_impl_1` `handle_key_event`: the trailing `settle` was unconditional, so a modifier press,
  a key typed into a text field (typed text arrives as a key *action*, not a key event), an arrow
  on an unfocused slider, or a stray release re-wrote every node of every page. Every branch that
  mutates state already returns its own `settle`; only a committed shortcut-recorder row needs the
  full settle, now flagged by `captured_key`. Expected shift: the largest slice of the input path
  (unhandled key events) drops from a full-document projection to zero writes.
- `app_impl_1` `sync_cursor_theme`: resolution stats `XCURSOR_PATH` theme directories on every
  settle. A new `applied_cursor_request` records the `cursorTheme` value that produced
  `applied_cursor_theme`, so the probe is skipped until that setting changes. Failures leave both
  caches empty, so the existing retry on the next settle is preserved. Expected shift: removes a
  handful of `stat` calls from every keystroke and action in the window.
- `app_impl_1` glass-slider and animation-speed drags: `Motion` settled the whole document at
  pointer rate. Motion now projects only the nodes the dragged value feeds
  (`sync_glass_rows` / new `sync_animation_speed`); `Release` still settles, and
  `sync_appearance` calls the extracted `sync_animation_speed` so there is one owner of that
  projection. Expected shift: the two live-drag paths leave the full-document projection; only
  their own value pill, fill and thumb are touched per pointer sample.
- `app_impl_2` `sync_nodes` navigation: the accent ink loop formatted `nav-<slug>` a second time
  for all 17 rows and resolved `solid_color` twice per sync. One pass now formats each row id once
  for visibility and label ink, and the accent colour is resolved once and shared with the
  selection background and `sync_accent_nodes`. Same writes, 17 fewer `String` allocations and one
  fewer parse per sync.

## Regressions reviewed

- `reproject` / `panels_refresh_changed`: a Panels wake with an unchanged snapshot skips the
  settle; settings, displays and shortcut scopes always settle, and the display-transaction signal
  carries no revision so it always projects. No bus path can now miss a real change.
- Startup/Defaults pages: `sync_nodes` still projects them only while their page is current, and
  `refresh` runs on every page switch, so the gated pages are current when shown.
- Theme pickers: rows and images are uploaded only while a picker is open, and
  `replace_image_for_node_shared` already no-ops on an identical source and `Arc`, so the
  remaining per-sync image writes do not re-upload.
- Key path: shortcut capture (`bare_super_binding`, chord rejection, Backspace/Escape), the display
  canvas keyboard path, focused sliders and sidebar-search Escape all return their own `settle`
  before the new tail, so none of them lost its projection.

## Skipped

- Idle wakeups: there is nothing to park. `poll_background` is called from the reactor callbacks
  and each poll is two `try_recv` calls plus a disabled-chooser check.
- Deferring projection while the window is obscured: X11 may unmap without a `Visibility` wake for
  this window, so a deferred settle could leave a stale first frame on reopen. Not worth the risk
  for a window that is only open on demand.
- Dead code in `flamewm-desktop` / `flamewm-explorer` from the cargo warning list: those paths
  belong to u41/u42; nothing provably unused was found inside the owned Settings paths, so nothing
  was deleted.
