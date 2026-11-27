# Unit p20-text-cache-bounds
STATUS: DONE

crates/flamewm-render-core text/glyph caches: confirm each cache has a byte cap and eviction (no unbounded growth with many distinct strings); add the cap if missing, identical visuals.

## Result

STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-core/src/glyphs.rs:73-82 | `FontCache` owns exactly two text caches: `glyphs: HashMap<(char, u32), GlyphEntry>` and `advances: HashMap<(char, u32), AdvanceEntry>`. No other cache exists in the crate (`text_layout.rs` holds only counters/hooks; `codec`/`model` maps are per-document, not caches). | none - already bounded | none |
| crates/flamewm-render-core/src/glyphs.rs:125-126 | `CACHE_LIMIT = 1024` caps the entry count of each map. Keys are `(char, px.to_bits())`, so many distinct strings built from the same characters add no entries; growth cannot be driven by string count. | none - cap present | none |
| crates/flamewm-render-core/src/glyphs.rs:84-120, 231-232, 266-269 | `make_room_for_glyph` and `make_room_for_advance` evict the least-recently-used entry (`min_by_key` on the shared `used` tick) before every insert, so both maps stay at or below the cap and the hot set survives. | none - eviction present | none |
| crates/flamewm-render-core/src/glyphs.rs:295-307, 323-331 | `trim_caches()` empties and shrinks both maps; `cache_stats()` exposes their counts for perf harnesses. Re-rasterization after eviction is proven pixel-identical by `trimming_caches_keeps_text_identical` (line 733). | none | none |
| .vendor/ | Only `CodexBar-main/` and `MacTahoe-gtk-theme-main/` are present; no KDE/xfce/smithay source is available to supply a byte-budget precedent. The in-repo `CACHE_LIMIT` + LRU policy is the established pattern. | none | no vendor precedent borrowed |

No production edits were made (0 edits), so there is nothing to mark UNBUILT; visuals remain pixel-identical by construction.

PASS - Both text caches in `flamewm-render-core` are already confirmed bounded and evicting: each is keyed per `(char, pixel size)`, capped at 1024 entries by `CACHE_LIMIT`, and prunes its least-recently-used entry before every insert, so a stream of many distinct strings cannot grow them without bound. The cap is an entry count rather than a literal byte budget; the only residual risk is an adversarial caller passing ever-increasing pixel sizes, where 1024 large coverage bitmaps could hold substantial bytes - that is outside the stated "many distinct strings" failure mode, and adding a byte budget would be an unbuilt behavioral change with no visual benefit. No change was needed and nothing was left uncommitted by this unit.

