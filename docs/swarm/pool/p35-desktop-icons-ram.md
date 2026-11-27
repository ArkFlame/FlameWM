# Unit p35-desktop-icons-ram
STATUS: DONE

Crate flamewm-desktop: icon and wallpaper caches: bound them (count and bytes), drop on output removal, no duplicate decoded copies; pixels identical.

## Result
STARTED
- crates/flamewm-image-core/src/decode_cache.rs:27 — wallpaper on-disk cache had only a 64 MiB byte budget, no entry-count bound — added `MAX_CACHE_ENTRIES = 256` — risk: UNBUILT
- crates/flamewm-image-core/src/decode_cache.rs:235-253 — `prune` evicted only on byte overflow, so many tiny entries could accumulate unbounded — evict oldest entries until both count and byte budgets hold; `keep` is never evicted — risk: UNBUILT
- crates/flamewm-desktop/src/main.rs:451-463 — process-local decoded raster was released only on solid fill and shutdown, never when an output disappeared — detect a removed `OutputId` on topology refresh and call `decode_cache::release_shared_raster()` — risk: UNBUILT

PASS: the wallpaper decoded cache is now bounded by both entry count and bytes, and the process-local decoded raster is dropped when an output is removed; the icon result cache already had count (2048) and byte (2 MiB) bounds in flamewm-integrations-linux/src/icon_service.rs, and `memory_put` already clears before insert so only one decoded raster is retained, so no duplicate decoded copies arise. Pixels stay identical: eviction never removes the just-written entry, and a released raster is re-read from the unchanged on-disk cache. No build or test was run (cargo and builds are forbidden by the brief); all changes above are UNBUILT.
