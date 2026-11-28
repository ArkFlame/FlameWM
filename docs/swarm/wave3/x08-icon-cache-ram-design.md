# x08 — Icon cache RAM: decoded-icon byte budget and lazy theme index

## Current structure (file:line)

- `crates/flamewm-integrations-linux/src/icons/mod.rs:92-116` — `Rgba8Raster`, the
  canonical decoded icon (`pixels: Vec<u8>` of straight RGBA8).
- `crates/flamewm-integrations-linux/src/icons/mod.rs:306-377` — `RasterCache`,
  the bounded decoded-raster LRU. `budget_bytes = 8 MiB` (line 326),
  `raster_bytes` = `pixels.len() + 64` (335-340), LRU order deque (342-349),
  oversize guard (358-360), eviction loop (361-368), `memory_estimate_bytes`
  (374-376).
- `crates/flamewm-integrations-linux/src/icons/tests.rs:531-559` —
  `oversized_raster_does_not_evict_or_escape_byte_budget` (budget test).
- `crates/flamewm-integrations-linux/src/icon_theme/index.rs:16-79` —
  `IconThemeIndex::parse`, full parse building `sections`, `dirs` and
  `dir_entries`; `index.rs:81-114` — `parse_inherits`, a lazy inheritance-only
  read; `index.rs:116-125` — `load` (full parse from disk).
- `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:156` — theme-chain
  walk uses `parse_inherits`; `walk.rs:178` uses full `parse` for the leaf.
- `crates/flamewm-integrations-linux/src/icon_theme/tests.rs:23-37` —
  `inherits_only_parse_matches_full_parse` (lazy-index parity test).
- `crates/flamewm-integrations-linux/src/icon_theme/caches.rs:19,44-51,107-113`
  — `EXISTS_CAP = 4096`, generation-scoped existence cache and byte estimate.
- `crates/flamewm-integrations-linux/src/icon_service.rs:145-150` —
  `RESULT_CACHE_ENTRIES = 2048`, `RESULT_CACHE_BYTES = 512 KiB * 4 = 2 MiB`.
- `crates/flamewm-integrations-linux/src/icon_service.rs:517-536` —
  `IconService::finish`, which posts decoded results into the shared cache;
  `icon_service.rs:539-573` — `ResultCache` with a running `bytes` total,
  `result_memory` and `empty_index`.
- `crates/flamewm-integrations-linux/src/icon_service.rs:1358-1395` —
  `result_cache_eviction_honours_byte_and_entry_limits` (original budget test);
  `icon_service.rs:1397-1418` — the new
  `oversized_result_is_not_cached_and_byte_bound_stays_absolute`.

## Counted problems

1. The shared result cache could break its own byte invariant. Before the
   step-1 edit, `finish` drained entries while `cache.bytes + cost >
   RESULT_CACHE_BYTES` and then inserted unconditionally. A single decoded
   1024x1024 RGBA result is `1024*1024*4 + 96 = 4,194,400` bytes, about 2.0x
   the 2,097,152-byte bound, so it emptied the cache and still left
   `cache.bytes > RESULT_CACHE_BYTES`, contradicting the assertion at
   `icon_service.rs:1381`. Count: one oversized decoded icon was enough.
   `RasterCache::insert` already had the matching guard (`icons/mod.rs:358`);
   the two decoded-icon caches disagreed.
2. Full `IconThemeIndex::parse` (`index.rs:18-79`) materialises a `sections`
   map plus two per-directory tables (`dirs`, `dir_entries`) even when the
   theme-chain walk only needs `Inherits` (`walk.rs:156`). For a theme with N
   declared directories that is O(N) string keys and cloned `IconDir` values
   per index read. The lazy `parse_inherits` path already removes this cost for
   chain traversal; only the leaf parse remains full by design.
3. The per-resolver `exists` cache is generation-scoped with `EXISTS_CAP`
   (`caches.rs:19`), so stale generations are freed before live entries are
   evicted — no unbounded growth there.

## Reference precedent (path + choice)

- `.vendor/MacTahoe-icon-theme-main/cursors/dist-dark/index.theme:1-5` — a
  real Freedesktop theme whose `[Icon Theme]` header carries `Name`, `Name[x-test]`
  and `Comment` keys while the directory sections live separately. This is the
  direct precedent for reading only `[Icon Theme]` header keys (`Inherits`,
  `Name`) during a chain walk instead of parsing every directory section.
- `.vendor/cursors-icons-themes-kdestore/` — large theme archives
  (`candy-icons.tar.xz`, `01-WhiteSur.tar.xz`, `01-Qogir-cursors.tar.xz`, ...)
  whose `index.theme` files declare many directory tables; parsing all of them
  for every inheritance hop is the RAM pressure this unit targets.
- Choice: port the vendor header-only read pattern, not the vendor assets.
  The pattern is implemented as `IconThemeIndex::parse_inherits`
  (`index.rs:83`) and used at `walk.rs:156`; full parse is retained at the
  leaf (`walk.rs:178`). No theme file is copied into production, no new
  dependency is added, and returned rasters are unchanged (pixel-identical).

## Target design

- Two independent decoded-icon byte budgets, both absolute: the per-resolver
  `RasterCache` (8 MiB) and the shared `ResultCache` (2 MiB). A decoded result
  larger than the whole budget is still returned to the caller but is not
  retained, so eviction of the working set can never be followed by a cached
  value above the bound.
- Theme-chain construction uses the lazy inheritance-only index; the full
  per-directory table is built only for the theme actually selected.
- Every cache exposes a running byte total (`result_cache_bytes`,
  `memory_estimate_bytes`) so the bound is testable without rescanning.

## Steps (each compiles alone)

1. **Implemented here (UNBUILT):** add the oversize guard to
   `IconService::finish` and a focused test. Changes only cache admission;
   `drain_ready`/`take_ready` still return the decoded raster. Compiles alone.
2. **Planned:** replace `ResultCache`'s arbitrary `keys().next()` eviction with
   the existing typed `BoundedLru` (`icon_theme/lru.rs:22`) so eviction is
   recency-based while the byte total stays exact. Compiles alone.
3. **Planned:** memoize the leaf `IconThemeIndex::parse` by
   `(theme_dir, metadata.modified)` so repeated full parses across theme
   changes avoid rebuilding `dirs`/`dir_entries`. Compiles alone.

## Rollback

- Step 1: remove the `if cost > RESULT_CACHE_BYTES { return; }` guard and the
  new test; no state or data migration, no runtime identity change.
- Step 2: restore the `keys().next()` eviction loop.
- Step 3: drop the leaf-parse memo and its invalidation.
- All steps keep visuals pixel-identical and add no dependencies.
