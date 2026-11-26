# u09 reactor/wm log

- widgets: `sync_widgets` compares the stored `SyncKey` field by field against live state instead of cloning the settings snapshot, widget list, weather, media and sample key on every call; an unchanged turn now allocates nothing (was one full `SettingsSnapshot`/`BTreeMap` clone per sync).
- widgets: `SampleKey` memory inputs are numeric and graph points compare against the raw history without rebuilding a scaled `VecDeque`; only raw-changed readings format text, so a skipped sync also skips four history clones and two `format_gib` allocations.
- slots: `project_slots` reuses one id buffer for the unchanged-tile visibility write (was one `format!` allocation per visible slot per sync), resolves the output grid once per output run instead of per slot, and returns `Cow` launcher icons so unchanged file tiles no longer clone their icon name each sync.
- slots: `apply_ready_icons` indexes model kinds once and passes the drained key for both identity sides (was `icon_count * SLOT_COUNT` re-walks of the model plus an `IconKey` clone per slot).
- downloads: `Organizer::tick` returns before any `symlink_metadata` unless new inotify notes arrived or the settle poll deadline is due; previously every pointer/key event re-stat'ed every pending download.
- main: `Resized`/`Visibility` controller events no longer run `refresh_all` before handling because the same turn's flush pass refreshes right after (removed a duplicate full projection per resize/obscure event).
