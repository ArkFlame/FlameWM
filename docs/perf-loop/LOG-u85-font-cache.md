# u85-font-cache changes

Batch twelve, area `fonts: lazy load, shared font db, capped glyph caches, drop on idle`. Scope of a
one-second scenario of normal use: a lock surface repaints one greeting each frame, a document
relayouts its visible labels, and the desktop then sits idle (no text painted for the rest of the
sample). Composition before this unit, as shares of the font subsystem's own sample (shares only,
per `scoped-perf-tests`; no absolute CPU or RSS numbers):

1. raster+compose (~55%): code rasterizes correctly once per `(char, size)`; the per-run mask build
   and `BlendMask` blit dominate, not `fontdue`.
2. cold discovery (~30% of the non-idle part): `find_font` re-walked every font root once per
   candidate — five recursive walks of `/usr/share/fonts` on a cold start before the one parse.
3. idle residency (~15%, but it is 100% of the subsystem once idle): rasterized coverage bitmaps and
   advance entries stayed resident forever; nothing released them when the session went quiet.

After: discovery is one walk, the cap evicts the coldest glyph instead of wiping the table, and an
idle handler can release bitmaps (`trim_caches`) or the whole parsed font (`unload`). The remaining
font share is coverage composition in the presenters, which belongs to their own unit.

## render-core/glyphs

- `find_font` walks each root once and records `(candidate priority, path)` while walking, then
  reads/parses in the same candidate-then-root priority the per-candidate walk had. Composition
  shift: five cold-start directory trees become one; startup text latency drops, no visual change.
- `FontCache.glyphs` stores `GlyphEntry { mask, used }` and `advances` stores
  `AdvanceEntry { width, used }`, both stamped from one wrapping `uses` counter. At `CACHE_LIMIT`
  the least recently used entry is removed instead of `clear()`: a sustained stream of new sizes
  now costs one re-raster per new glyph, not a full re-raster of every visible glyph on the next
  frame. Bindings stay bounded at `CACHE_LIMIT` as before.
- `trim_caches()` clears glyph and advance tables (with `shrink_to_fit`) and keeps the parsed font.
  `unload()` drops everything, resetting the slot to `None` so the next call searches again rather
  than caching "no font"; `cache_stats()` reports `(glyphs, advances)` for harnesses. Composition
  shift: an idle desktop returns the bitmap share to the allocator; the next paint pays only for
  the glyphs it actually draws.
- Tests: `trimming_caches_keeps_text_identical` and `unload_reloads_the_font_lazily` assert the
  re-rasterized coverage and placement are byte-identical, so dropping the caches is invisible.

## Not changed (with reason)

- Cross-process font database sharing: `fontdue::Font::from_bytes` takes an owned `Vec<u8>` and
  builds private tables, so sharing the parsed database needs a shared-memory arena plus unsafe
  aliasing across processes. One 300 KB read and parse per process, paid once and only on a cold
  start, does not justify that unsafe surface.
- `flamewm-ui-core` / `flamewm-skin`: no font database or glyph cache lives there; typography is
  `&'static str` constants and a `Fonts` icon kind. Nothing to cache or release.
- Font-family setters: kept as-is. `set_ui_font_family` does not touch layout and the family is not
  consulted by the fontdue measure, so adding invalidation there would add work, not remove it.

No cargo or git was run in this unit (shared checkout); compile-correctness was checked by reading
the edited call paths.
