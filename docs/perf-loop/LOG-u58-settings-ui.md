# u58-settings-ui

Owned paths: `crates/flamewm-settings*/src/**` (git is forbidden in this shared checkout, so the
recent-commit review below reads the current projection code and the tests that guard it instead
of `git log -p`).

Composition read: Settings still has no timer and no frame tick. Its cost is the event path:
every pointer sample, key and reactor wake runs one `settle`, which projects the whole document.
Within that path the ranked costs are (1) blocking control transactions caused by live drags,
(2) per-settle node-id and value formatting for nodes nobody can see, (3) repeated snapshot work
whose inputs did not change. This unit attacks those three, plus one cache regression.

## Changes

- `app_impl_1` animation-speed and glass-slider drags: every pointer `Motion` used to send a
  synchronous `apply_setting` transaction (blocking control round trip, snapshot replacement and
  a bus echo that wakes this same window into another settle). Writes are now coalesced to one
  per `SLIDER_WRITE_INTERVAL_MS` (120 ms) while the drag preview keeps the pill, fill and thumb
  live on every sample, and `Release` always writes the final value. Expected shift: a drag drops
  from one blocking transaction plus one echoed settle per sample to roughly 8 writes per second
  and one settle at the end. `execute` and `Release` clear the preview so no stale value survives
  the gesture.
- `app_impl_3` `sync_animation_speed` / `sync_glass_rows`: read the drag preview before the
  authoritative snapshot, so the coalesced write never makes the control lag the pointer.
- `app_impl_2` `sync_theme_pickers`: a closed picker used to format and rewrite all 32 row
  visibilities for each of the two kinds on every settle — 64 id strings and 64 writes while
  nothing is on screen. The rows now hide once on the settle that closes a picker (tracked in
  `open_picker_rows`) and a closed picker costs one visibility write and no row churn.
- `app_impl_2` `sync_nodes`: the appearance and nav-row token projection walks the document's
  colour variables from the settings snapshot. It is now keyed on the snapshot revision, so
  input, page switches and drags with an unchanged snapshot keep the variables already in the
  document. Expected shift: removes that variable pass from every settle except a real settings
  change (a bus refresh or a committed write).
- `app_impl_2` output cards: the visual order is sorted once per settle instead of once per slot
  through `output_page_slot`, and the output count is read once for the pager. Same rows, two
  fewer sorted `Vec`s per settle.
- `app_impl_3` `sync_glass_rows`: dropped the always-`Some` wrapper around the taskbar material
  mode; the option is compared directly.

## Regression fix

- `app_impl_1` `sync_cursor_theme`: u43 keyed the skip on `applied_cursor_theme.is_some()` **and**
  the matching request. When `cursor_theme_reload` returns `None` (the applied theme already
  equals the resolved request) the request was cached but the theme stayed `None`, so the guard
  never matched and every settle re-statted `XCURSOR_PATH` again. The guard now trusts the
  resolved request alone; a failed `apply_cursor_theme` clears it so the retry behaviour is kept.

## Regressions reviewed

- Slider throttling: every branch that commits a value (`Release`, keyboard steps through
  `execute`, steppers) clears the preview and writes unconditionally, so a pointer that leaves
  the window without a final sample still keeps the last throttled value committed within
  120 ms and cannot strand an unapplied preview past the next action.
- Theme picker rows: the close settle hides every row unconditionally before the fast path
  resumes; reopening rebuilds names, previews and images as before. Tests assert no row state, so
  the only contract (row ids and preview ids) is unchanged.
- Token cache: a settings snapshot revision change (refresh, apply, bus signal) re-projects the
  tokens, and a new `SettingsApplication` starts with no cached revision. Tests that build a new
  document with a reused application did not assert token writes.
- Display order: `ordered_output_indices` is unchanged and still used by the input path; only the
  per-slot call in the projection loop was replaced by one shared order.

## Skipped

- Page-gating the Displays/Hotkeys/Assistant projections: the strict-document tests and the
  display-canvas tests deliberately assert those nodes are projected from a non-Displays page, and
  X11 may unmap without a visibility wake, so a deferred first frame could show stale content.
  Not worth the risk for a window that is only open on demand.
- Deleting dead code: no provably unused item was found inside the owned paths; the only
  needless abstraction found (`sync_glass_rows`' `Some(...)` mode) was removed.
