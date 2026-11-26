# u37 compositor (flamewm-desktop projection) log

Area: `crates/flamewm-desktop/src/**` — the desktop's own projection path, which is what this
process contributes to every compositor frame it is part of: full-root window, so any redundant
document write here becomes damage for the compositor, and any per-sync allocation here is UI-thread
work for the whole desktop. Scouted by composition share: a per-flush/refresh cost multiplies by
every interaction and every inotify burst, while the once-per-generation costs (wallpaper decode,
icon index) are already off-thread.

## Review of the recent slices (u22, u30)

No feature regression found in the defers added for covered windows, pointer-motion pre-refresh
skips, staged drag/sticky gestures or the icon hover cache. One latent bug fixed:

- `menus::sync_menu` always ran `hide_menus` first. `hide_menus` clears `menu_rects`, and the open
  delete confirm keeps its only press rect in that same vector, so the first `sync_document` after
  a confirm opened (e.g. an inotify rescan) silently disarmed the outside-press close path while
  the modal stayed on screen. The hide pass is now skipped when no menu is shown, and an armed
  `pending_delete` keeps its rects.

## Optimizations

- menus (`sync_menu`): an ordinary desktop has no menu open and no rect placed. Showing that state
  already implies the nodes are hidden, yet every `sync_document` issued eight `visible(false)`
  writes, seven backdrop clears and a `menu_rects.clear()` that also dropped the confirm rect. An empty `menu_rects` with no owner now returns before any write. Composition shift: the
  menu share of a selection/drag/rescan sync drops from a constant write burst to zero.
- widgets (`SyncKey`): the key stored a fully formatted `Placeholders` (username, hostname, time,
  date, weekday, cpu/mem text, uptime) built before the unchanged check, so every interaction sync
  allocated and formatted ~7 strings even when the key matched. `cpu/mem/uptime` are already in
  `SampleKey` numerically and `time/date/weekday` derive from `time`, so the key now stores the raw
  username/hostname plus those existing inputs, and `widget_placeholders` runs only after the
  decision to sync. Deleted `placeholders_match` (dead). Composition shift: unchanged widget turns
  read state and allocate nothing; a settings change no longer formats date/time twice.
- sticky (`StickyProjection`): `sync_sticky` rewrote every visible note on every sync — position,
  size, background, text, font size/weight, foreground, 5 visibility nodes, with a `format!` per id
  per note — even when triggered by an unrelated change (menu open, selection, rescan). It now
  captures the drawn inputs (visible notes, enabled gates, gesture preview, edit buffer) and
  returns before any write when they match. `visible_sticky_notes` is no longer cloned per sync;
  the borrowed note slice drives the comparison. Composition shift: sticky share of unrelated
  syncs drops from O(notes) document writes to O(notes) field compares; only real note changes
  repaint.
- projection (`SlotIds`, `sticky::StickySlotIds`): tile and note node ids were rebuilt with
  `format!` on every reprojection — ~17 strings per tile (item, 5 glyphs, label, 2 lines, 5 glyph
  applies) and 6+4 per sticky note. Ids are now formatted once into a lazily built static table and
  returned as `&'static str`. `rubber::RubberItem.slot` holds the shared id instead of a `String`.
  Composition shift: a full 128-tile rescan removes ~2000 short allocations; steady syncs remove
  the remaining per-note id churn.
- slots (`project_slots`): the per-sync `Vec` of item references is replaced by one live model
  iterator (index order is identical), removing the last allocation from an unchanged slot pass.
- drag (`sync_drag_ghost`): a ghost whose icon key never resolves (unknown theme name) re-faded and
  re-uploaded the 44px RGBA fallback on every flush of a drag. The swap now only runs once the
  cache actually holds the raster; a miss is free and the painted fallback stays.

## Checks

No cargo and no git in the shared checkout. All edited files parse and are format-clean under
`rustfmt --edition 2024 --check`; the two new unit tests (`slot_ids_are_cached_and_cover_every_glyph_kind`,
`sticky_projection_key_tracks_notes_gates_preview_and_edit`) are compile-reviewed only. Visuals and
behavior unchanged: same ids, same written values, same fallback pixels.
