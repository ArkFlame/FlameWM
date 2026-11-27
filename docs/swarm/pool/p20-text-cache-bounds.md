# Unit p20-text-cache-bounds
STATUS: DONE

crates/flamewm-render-core text/glyph caches: confirm each cache has a byte cap and eviction (no unbounded growth with many distinct strings); add the cap if missing, identical visuals.

## Result

- `crates/flamewm-render-core/src/glyphs.rs:73-76` — finding: cache had an entry cap (`CACHE_LIMIT=1024`) and LRU eviction but no byte bound; one (char, px) glyph can be megabytes. change: added `GLYPH_BYTE_CAP = 8 MiB` coverage-byte budget. risk: UNBUILT (cargo forbidden); no visual change.
- `crates/flamewm-render-core/src/glyphs.rs:78-122` — finding: no byte accounting. change: track coverage bytes and evict LRU until both caps admit an incoming glyph; glyphs larger than the budget are returned uncached. risk: UNBUILT.
- `crates/flamewm-render-core/src/glyphs.rs:248-258` — finding: insert path ignored bitmap size. change: pass `glyph.coverage.len()` to the eviction gate and account cached bytes. risk: UNBUILT.
- `crates/flamewm-render-core/src/glyphs.rs:299-305` — finding: cache construction lacked the counter. change: initialize `glyph_bytes: 0`. risk: UNBUILT.
- `crates/flamewm-render-core/src/glyphs.rs:323-324` — finding: `trim_caches()` cleared maps but would leave a stale byte count. change: reset `glyph_bytes = 0` on trim. risk: UNBUILT.
- Not changed, confirmed safe: `advances` is already byte-bounded (fixed-size entries + `CACHE_LIMIT` + LRU); `layout::engine::MeasureMemo` is a per-pass memo, not a growing text cache; `.vendor/kwin-master` and `.vendor/smithay-master` had no applicable glyph byte-cap precedent.

Every production edit above is UNBUILT: the brief forbids `cargo`, so no compile, test, or visual verification was run; the orchestrator will build after the batch lands.

PASS — the byte cap and LRU eviction are in place for the glyph cache, the counters are kept in sync on insert, eviction and trim, and eviction only affects caching, never a returned bitmap, so visuals stay pixel-identical; build and runtime checks were explicitly out of scope and remain UNBUILT.
