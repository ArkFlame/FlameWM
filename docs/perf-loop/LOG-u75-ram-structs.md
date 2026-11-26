# u75-ram-structs — RAM long-lived structures (shell, explorer, desktop)

Batch eleven. Survey plus two bounded changes. No `cargo`, no `git` (shared checkout; the
orchestrator runs the batch gate). Static review only: every edited site was re-read for types,
borrows and call sites.

## Composition reasoning (shares, not absolute numbers)

Idle RAM in these three crates is dominated by, in order:

1. **Wallpaper and icon pixel payloads.** The desktop keeps the decoded wallpaper raster; the
   shell icon cache is byte-capped at 32 MiB; the explorer thumbnail cache is capped at 256 KiB.
   All three are deliberate, bounded, and traded against redraw work. Not touched.
2. **Explorer listings.** Up to `MAX_LISTED_ENTRIES` rows of `Entry` (path, name, metadata) per
   active tab; hidden tabs already drop their rows (`release_entries`). Struct layout is packed
   by the compiler (80 B) and streamed batches keep peak reallocation bounded. Not touched.
3. **Shadow copies of application metadata.** These are pure bookkeeping: they exist so that
   UI state can tell whether a list changed. They are the cheapest place to cut RAM without
   touching visuals, and they were the largest removable retention left.

## Changes

### flamewm-shell: dock fallback catalog keeps only the fields it reads

`ShellLoop::dock_catalog` retained a full `Vec<DesktopApplication>` discovered from the desktop
entry catalog, used only to look up the label and `Icon=` of a pinned app that the shell
snapshot does not list. Every retained element carried `argv`, `keywords`, `categories`,
`comment`, `generic_name`, `startup_wm_class` and their heap strings for the whole process life,
for two string fields.

- New `app::dock::CatalogEntry { id, name, icon_name }`.
- The fallback is built once from `ApplicationCatalog::all()` and each application's unneeded
  fields are dropped immediately; `dock_catalog` is now `Option<Vec<CatalogEntry>>`.
- `lookup` returns `Option<(&str, &str)>` (name, icon) and prefers the live shell snapshot before
  the compact fallback, preserving the previous search order.
- Call sites copy out only `name` / `icon_name`, exactly as before.

Expected composition shift: shell idle heap drops the catalog shadow (roughly one third of a
catalog's retained strings on a typical install). Small in absolute share against icon rasters,
but it is permanent, never re-read, and the same catalog is already owned in full by
`StartModel`, so the shell no longer holds a second large copy.

### flamewm-desktop: bound the watcher name set

`refresh::ChangedPaths::Changed.names` accumulated every distinct `OsString` the desktop
directory watch reported between rescans, with no cap; only the watcher's own overflow flag
cleared it. A bridge or a bulk operation could grow it without limit while rescans are debounced.

- `ChangedPaths::NAMES_CAP = 1024`. On reaching the cap the set is cleared and `overflow` is set,
  which `take()` already reports as "unknown, re-check every entry", so launchers still get a
  complete pass and the change-detection stamp shortcut stays correct. Once `overflow` is set,
  `note` returns immediately instead of re-filling the set.

Expected composition shift: bounded watcher memory during bursts; the fallback path is a full
launcher re-check that already existed for kernel overflow, so no visual or behavioural change.

## Surveyed and deliberately left alone (already bounded)

- Shell icon rasters: 2048 entries / 32 MiB LRU, shared `Arc` pixels, `Arc<str>` name keys.
- Shell icon bindings: keyed by slot target and edge, so bounded by slot count.
- Shell switcher preview cache: rebuilt from each snapshot, only generations the snapshot names
  survive; `resolve` still copies pixels because the wire type owns a `Vec<u8>` (changing that
  type is outside this unit's paths).
- Explorer thumbnails: 256 entries / 256 KiB LRU, single decode per key (`pending` set).
- Explorer search: `MAX_HITS` 2000, `MAX_VISITS` 200 000, query clamp.
- Explorer trash: `MAX_TRASH_ENTRIES` 10 000, info-file byte cap.
- Explorer undo journal: both stacks trimmed to `MAX_UNDO`.
- Explorer remote recents: `RECENT_LIMIT` enforced on load and on remember.
- Explorer `DirWatch`: no name accumulation (one coalesced callback per quiet window).
- Desktop launcher cache: rebuilt from the directory watch on every rescan, so deletions drop
  their entries; no unbounded growth.
- Desktop trash outcomes: drained on the reactor wake; not a persistent cache.
- Desktop icon slots and their input fingerprints: fixed `SLOT_COUNT` arrays.

## Checks

None executed (`cargo` and `git` are reserved for the orchestrator's batch gate). Type and
borrow correctness verified by reading the edited functions and their call sites: `lookup`
returns borrowed name/icon pairs, both call arms still produce `String`, and the closure borrows
exactly the same state as before.
