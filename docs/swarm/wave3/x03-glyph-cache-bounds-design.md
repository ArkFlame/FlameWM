# x03 design: bounded glyph and text raster cache

Route evidence: workspace version 0.1.7 (`Cargo.toml:5`; `scripts/version` reads the same value
through Python, which this brief forbids, so the file was read instead). Routes from
`.opencode/ROUTES.md`: source-first (callers and owners), reference-mining (`.vendor/**` evidence),
rust-systems (ownership and native boundary). The task brief overrides skill read budgets, so no
route skill was loaded for this unit. No build, Cargo, Git or Python command was run.

## Current structure (file:line)

All production code lives behind the `glyphs` facade of `crates/flamewm-render-core`.

- `src/glyphs.rs:35-46` — `GlyphMask` (8-bit coverage bitmap plus placement).
- `src/glyphs.rs:50-59` — `TextMask` (assembled run).
- `src/glyphs.rs:62-65` — `GlyphEntry { mask, used }`; `src/glyphs.rs:68-71` — `AdvanceEntry`.
- `src/glyphs.rs:76` — `GLYPH_BYTE_CAP = 8 * 1024 * 1024`.
- `src/glyphs.rs:78-90` — `FontCache { font, glyphs, advances, uses, glyph_bytes }`.
- `src/glyphs.rs:95-112` — `evict_for_glyph`, the LRU plus byte-cap policy (step 1 of this design).
- `src/glyphs.rs:115-127` — `FontCache::tick` and `FontCache::make_room_for_glyph` delegation.
- `src/glyphs.rs:129-140` — `make_room_for_advance` (entry cap only; fixed-size entries).
- `src/glyphs.rs:238-262` — `Run::glyph`: lookup, rasterize, admit, insert.
- `src/glyphs.rs:280-296` — `Run::advance`: shared glyph/advance lookup, no rasterization.
- `src/glyphs.rs:322-335` — `trim_caches`; `src/glyphs.rs:337-349` — `unload`.
- `src/glyphs.rs:355-359` — `cache_stats() -> Option<(usize, usize)>`.
- `src/glyphs.rs:803-879` — unit tests added in step 1.

## Counted problems and gaps

- Eviction tests before this unit: **0**. `rg` found `make_room_for_glyph` at only its definition
  and one call site (`src/glyphs.rs:255`), with no test exercising it.
- Byte-cap observability: `cache_stats` returns a **2**-tuple (glyph entries, advance entries);
  `glyph_bytes` is not exported, so a perf harness can see entry counts but not the byte budget.
- Testability: the policy was a `FontCache` method, so any test needed a parsed `Font` and silently
  skipped on hosts without one. Step 1 moves the policy to a pure function over the table and its
  byte counter; no font is parsed by the new tests.

## Reference precedent and choice

- `.vendor/RustWebRender-0.0.9/crates/rustwebrender-x11/src/lib.rs:91-113` and `:769-873`:
  `ImageCacheKey { asset, width, height }` keyed `HashMap`s, lookup before creation, insert after.
  **Choice:** port the keyed lookup/lazy-insert shape (here `(char, px-bits)`) but not its unbounded
  map; the bound is added locally as LRU plus coverage-byte cap. No verbatim code was copied.
- `.vendor/starling-main`: `rg` over `*.rs` for glyph/text/raster cache returned no matches.
  **Choice:** no code ported from starling; it has no glyph raster cache to borrow.

## Target design

Keep the typed `glyphs` facade as the sole owner of raster caches. A glyph entry holds a coverage
bitmap and a last-use tick; a monotonic counter stamps every hit. On admission, evict the coldest
entries until both `CACHE_LIMIT` entries and `GLYPH_BYTE_CAP` coverage bytes admit the incoming
payload; an oversized glyph is returned to the caller but never cached, so a single huge size
cannot break the budget and pixels stay identical. Advances share the tick but use a fixed-size
entry table capped by `CACHE_LIMIT`. `trim_caches` clears both tables and their byte counter,
`unload` drops the parsed font too, and `cache_stats` exposes the bound for perf harnesses.

## Steps (each compiles alone)

1. **This unit.** Extract the eviction policy into `evict_for_glyph` (pure, font-free) and add
   deterministic tests for coldest-first eviction, repeated byte eviction, entry-cap eviction and
   oversized-glyph rejection. Behavior is unchanged; UNBUILT here.
2. Widen `cache_stats` to `(glyph_entries, advance_entries, glyph_bytes)` and update any perf
   harness that consumes it (none in `crates/` today). Compiles alone with the step 1 base.
3. Replace the linear `min_by_key` scan with an explicit least-recently-used order structure only
   if a scoped perf test shows the scan is the largest share of a glyph-heavy frame. Compiles
   alone; visuals remain pixel-identical because only eviction order ties change.

## Rollback

Step 1 reverts by restoring the inline body of `FontCache::make_room_for_glyph` from
`src/glyphs.rs:102-122` of the pre-change file and deleting `evict_for_glyph` plus the four tests;
no other module imports the helper, so nothing else changes. Steps 2 and 3 revert independently by
restoring the previous `cache_stats` signature or the linear scan. No new dependencies are added.

## Step 1 result (UNBUILT)

Production edit: one refactor at `src/glyphs.rs:95-112` and `:124-126` — the eviction body moved
verbatim into `evict_for_glyph` and the method now delegates. No cap, tick, insertion order or
raster path changed; pixels stay identical. Tests at `src/glyphs.rs:803-879` construct synthetic
`GlyphEntry` values with `Arc<GlyphMask>` coverage vectors, so they run without a font and exercise
the real production helper. All edits are **UNBUILT**; no Cargo command was permitted in this unit.
