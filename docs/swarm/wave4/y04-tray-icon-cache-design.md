# y04 — Tray icon cache and StatusNotifier watcher absence

## Current structure (file:line)

- `crates/flamewm-tray/src/item.rs:148-154` — `IconPixmap { width, height, argb: Vec<u8> }`,
  owned ARGB32 bytes (network order, row-major).
- `crates/flamewm-tray/src/item.rs:168-179` — `ItemProps`, including
  `icon_pixmaps: Vec<IconPixmap>` at line 178; the string and pixmap buffers are moved
  out of the `GetAll` reply by `from_owned_props` (`item.rs:194-226`).
- `crates/flamewm-tray/src/watcher.rs:27-29` — the only icon bounds today:
  `MAX_PIXMAPS = 8` entries per item and `MAX_PIXMAP_BYTES = 256 * 256 * 4` (262,144 B)
  per entry.
- `crates/flamewm-tray/src/watcher.rs:529-570` — `pixmaps()` decoder: per-entry size check
  (line 552), fresh `Vec<u8>` allocation (line 555), `.take(MAX_PIXMAPS)` (line 568).
  No cross-item byte total and no reuse of an identical buffer.
- `crates/flamewm-tray/src/watcher.rs:286-307` — `Pending::Props` reply replaces the whole
  `ItemProps` through `Registry::set_props`; old pixmap buffers are dropped, not cached.
- `crates/flamewm-tray/src/registry.rs:8,29-35,61-72` — `MAX_ITEMS = 64`,
  `items: BTreeMap<ItemKey, ItemState>` and `set_props`; the registry is the sole
  retention point for decoded icons.
- `crates/flamewm-shell/src/tray_host.rs:148-169` — `pixmap_to_image` re-hashes the entire
  ARGB buffer on every projection and bakes the hash into `RuntimeImage.source`
  (line 159-164); `:385` posts it with `document.image_rgba8`.
- `crates/flamewm-shell/src/tray_host.rs:181-202` — `TrayHost::connect` returns `None`
  when `TrayWatcher::connect()` errors; `crates/flamewm-tray/src/watcher.rs:84-88`
  propagates `pump.own_name(WATCHER_NAME)?` when another watcher already owns the name.

## Counted problems

1. No aggregate icon-RAM bound. The caps at `watcher.rs:27-29` are per item, not global:
   64 items (`registry.rs:8`) × 8 entries × 262,144 B = 134,217,728 B (128 MiB) of
   retained ARGB, every byte allocated fresh at `watcher.rs:555`. Count: one worst-case
   item set reaches the 128 MiB ceiling; nothing enforces a smaller budget.
2. No stable icon identity. `pixmap_to_image` (`tray_host.rs:149-163`) hashes the full
   buffer per projection and derives `source` from it, so the same icon bytes from two
   items or two `NewIcon` updates become two document images and two `Vec<u8>` copies.
   Count: one extra full-buffer copy per dirty projection per item.
3. Hard fail on watcher absence. `TrayWatcher::connect` (`watcher.rs:85-86`) turns a
   foreign `org.kde.StatusNotifierWatcher` owner into an error, and
   `TrayHost::connect` (`tray_host.rs:184-201`) maps it to `None`. Count: one existing
   watcher owner disables all `SLOT_COUNT = 6` tray buttons, the chevron and tray menus.

## Reference precedent (path + choice)

- `.vendor/vela-main/shell/src/tray.h:88,103-106` — `QHash<QString, QImage> m_pixmaps`
  keyed by "address/serial", read through `pixmap(const QString& key)`, exposed to QML
  by `TrayImageProvider` (`tray.h:114-122`, `image://tray/<key>`): one stable keyed image
  map instead of per-projection buffers. `tray.h:10-13,39` also records the graceful
  rule: if nobody offers the watcher, the shell provides it; `registerService()`
  reports `false` rather than aborting when another registry exists.
- `.vendor/LingmoOS-main/core32/applets/systemtray/statusnotifieritemhost.cpp:60-65,68-79,81-123,125-144`
  — a `QDBusServiceWatcher` follows the watcher name; an invalid watcher is deleted,
  set to `nullptr` and logged ("System tray daemon not reachable") without failing the
  host; disappearance disconnects, clears services and nulls the pointer; a returning
  owner re-registers. No hard failure for a missing tray daemon.
- Choice: port the **stable keyed pixmap map** (Vela) and the **absence-as-state with
  retry** rule (Lingmo), re-expressed with flamewm types as `IconCacheKey`/`IconCache`
  and a planned `WatcherAvailability`. No vendor code or assets are copied into
  production, no dependency is added, and ARGB bytes are stored and returned
  unchanged, so rendered pixels stay identical.

## Target design

- `flamewm-tray::icon_cache`: an absolute byte budget (4 MiB) for retained ARGB, a
  stable key built from dimensions plus a content fingerprint, LRU eviction, exact
  running byte total (`memory_estimate_bytes`) and an oversize rule matching
  `docs/swarm/wave3/x08-icon-cache-ram-design.md` — an entry larger than the whole
  budget is handed to the caller but never retained.
- Step 2 feeds decoded pixmaps through that cache once and keeps keys in `ItemProps`;
  `pixmap_to_image` derives a document `source` from the cache key, so repeated
  projections share one image instead of copying the buffer.
- Step 3 models watcher presence as a typed `WatcherAvailability` (`Absent` / `Active` /
  `Fallback`) refreshed by `NameOwnerChanged`, so the shell keeps a fallback-glyph tray
  instead of returning `None` when another watcher owns the name.

Blast radius: only `flamewm-tray` internals and the `flamewm-shell` tray projection
(`tray_host.rs`) consume these types; no WM authority, reactor ownership or X11
coordinate/grab rule changes. UI remains a typed projection, never an authority.

## Steps (each compiles alone)

1. **Implemented here (UNBUILT):** add the pure `flamewm-tray::icon_cache` module with
   byte-bounded LRU admission, a stable key and focused tests, plus its `lib.rs`
   re-export. No call site changes yet; existing `ItemProps` still carry raw pixmaps.
   Compiles alone and adds no dependency.
2. **Planned:** route `watcher.rs` decoded pixmaps through `IconCache`, hold cache keys
   in `ItemProps`, and key `pixmap_to_image` sources by `IconCacheKey`. Compiles alone.
3. **Planned:** add typed watcher-availability + retry (Lingmo precedent) and make
   `TrayHost::connect` retain a fallback host instead of `None`. Compiles alone.

## Rollback

- Step 1: delete `icon_cache.rs`, its `lib.rs` re-export and the tests; no state or
  data migration, no runtime identity change, no visual change.
- Step 2: restore raw `Vec<IconPixmap>` in `ItemProps` and buffer-hash image sources.
- Step 3: restore `own_name(...)?` propagation and the `None` return in `TrayHost`.
- All steps keep visuals pixel-identical and add no dependencies.
