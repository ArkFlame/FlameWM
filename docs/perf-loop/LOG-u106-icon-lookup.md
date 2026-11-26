# u106-icon-lookup

Area: icon theme lookup (Freedesktop index). The lookup snapshot already builds
one `IconLookupIndex` lazily (OnceLock scan), caches negative names in a bounded
`NameMemo`, and resolves from the scan with no per-draw path probing; the
`IconResolver` request/raster caches absorb repeat draws. Remaining work was the
cold-path constant overhead per resolved name and the `exists` cache holding
dead-generation keys.

Changes:
- `icon_theme/index.rs`: the parsed `index.theme` now also stores `dir_entries`
  (`Vec<Option<IconDir>>` in `Directories` order). Directory metadata is
  addressed by `directory_rank`, so lookup no longer hashes a directory name.
- `icon_theme/lookup.rs`: `lookup` now
  - normalizes through `normalize_parts` (`Cow<str>`), so a bare icon name no
    longer allocates a `String` just to binary-search the file index;
  - hoists the `IconFileIndex`/theme-chain handles once per lookup instead of
    re-entering the `OnceLock` and hashing the theme name per candidate file;
  - resolves distances from the rank-indexed `dir_entries` (replaces the
    `directories` + `dirs` double hash lookup per candidate file);
  - keeps per-theme bests in a rank-indexed slot vector, removing the
    per-lookup `HashMap` and the `sort_by_key` that only re-derived chain order
    (precedence, exact-size-first ordering, and pixmap fallback unchanged).
- `icon_theme/caches.rs`: at the `EXISTS_CAP` bound, an unreachable generation
  (a snapshot bump makes its answers unreachable) is freed whole before any
  live entry is evicted, so a theme change keeps the current generation's hit
  rate and stale keys do not sit at the cost of live ones.

Tests: added `dir_entries_align_with_directories_by_rank` (rank table mirrors
declared directories, holes stay holes) and
`exists_cache_frees_a_dead_generation_before_live_entries` (cap pressure frees
the dead generation, live entries survive). Existing memo and cache bound tests
unchanged.

Expected composition shift: icon cold resolve per name drops its per-lookup
hash work (theme-name hash per file, directory-name hash per file, `HashMap` +
sort of per-theme bests) and one `String` allocation; theme changes stop paying
a slow loss of `exists` hit rate to stale generation keys. Warm draws were
already memory-only and are unchanged. No visuals or features touched
(precedence, formats, pixmap fallback, and inheritance chain preserved).
