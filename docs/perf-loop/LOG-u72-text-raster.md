# u72-text-raster changes

Batch ten, area `text raster and shaping`. Scope of a one-second scenario of normal use: one shell
panel republishes text (clock, status, labels), a document update lays out its visible labels, and
the lock surface paints one greeting. Composition estimate before this unit, by share of the text
subsystem's own sample (per `scoped-perf-tests`, shares only, no absolute CPU):

1. shaping/measurement (~45%): every word and every wrapped candidate line called
   `glyphs::advance_width`, which re-ran `fontdue::Font::metrics` per char per call. A greedy
   20-word paragraph measured ~40 strings, each re-scaling each char.
2. line assembly (~30%): the greedy fill rebuilt the running line with `format!` once per word, and
   `push_chunks` collected a `Vec<String>` plus a `Vec<char>` per overlong token.
3. mask build (~25%): `text_mask` rasterized whitespace into empty glyph entries, held a third
   `Vec` of `(origin, glyph)` pairs, then walked it twice.

After: measurement is a hash lookup per char, line assembly writes into one reused buffer, and the
mask build keeps two vectors and rasterizes only inkable chars. The remaining text share is the
per-pixel `BlendMask` span, which is already `div255` table-driven.

## render-core/glyphs

- `FontCache` gains a bounded `advances` table keyed by `(char, size bits)`. `Run::advance` answers a
  painted char from its glyph entry (same value `shape` uses), then from `advances`, and only then
  from `fontdue::metrics`. Composition shift: wrapped-layout measurement stops re-scaling glyph
  metrics per word; the two paths cannot disagree because the glyph entry wins.
- `shape` leaves whitespace as `None` and advances it through `Run::advance` instead of rasterizing
  an empty coverage mask into the glyph cache. Composition shift: ~15% of chars in normal English
  text no longer allocate a glyph entry, and no longer push the 1024-entry cache toward its reset.
- `text_mask` drops the intermediate `Vec<(i32, Arc<GlyphMask>)>`: one pass over the zipped
  origins/glyphs finds the ink bounds, a second pass blits, so one allocation per painted run is
  gone and the empty-run early return is unchanged.

## render-core/text_layout

- Greedy word fill appends `' ' + word` to the running line, measures that whole string, and
  truncates back when it does not fit. Previously `format!("{current} {word}")` allocated a fresh
  line buffer per word. Text output is byte-identical: the candidate string measured is still the
  exact painted line, and the emitted line is still the pre-word prefix.
- `push_ellipsized` probes with one reused candidate buffer and tracks the best cut, then builds the
  final ellipsized line once. Previously every binary-search probe allocated a new prefix string;
  the chosen cut and its measured width are unchanged.
- `push_chunks` replaces `char_chunks`: chunks are pushed straight into `lines` from a byte-offset
  table, deleting the `Vec<String>` result, the `Vec<char>` copy of the token and per-chunk
  `iter().collect()` rebuilds. Chunk boundaries and widths are the same caret-driven ones.

## Deleted

- `char_chunks` and the `candidate` closure in `push_ellipsized` had no other caller.

Expected composition shift: text shaping/measurement and line assembly shares drop; mask build
drops one allocation plus whitespace rasterization; visuals and layout results are unchanged.

UNVERIFIED: no cargo/git in this shared checkout; `rustfmt --check` parses both edited files and
reports only two pre-existing assertion-formatting diffs in untouched test code. The orchestrator
runs the build, tests and commit.
