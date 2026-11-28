# y01 design: bounded explorer thumbnail cache

Route evidence: workspace version 0.1.7 (`Cargo.toml:6`; `scripts/version` reads the same value
through Python, which this brief forbids, so the manifest was read instead). Routes from
`.opencode/ROUTES.md`: source-first (callers and owners), reference-mining (`.vendor/**` evidence),
rust-systems (ownership and native boundary). The task brief overrides skill read budgets, so no
route skill was loaded for this unit. No Git, Cargo, Python or build command was run; all edits
are UNBUILT.

## Current structure (file:line)

Everything is owned by the `thumbs` facade of `crates/flamewm-explorer`; other crates do not import it.

- `src/thumbs.rs:28` — `THUMB_SIZE = 18`; `:30` `MAX_SOURCE_BYTES` (25 MiB); `:32` `MAX_SOURCE_PIXELS` (25M).
- `src/thumbs.rs:34` — `CACHE_CAPACITY = 256`; `:36` `CACHE_BYTES = 256 * 1024`; `:39` `HIDDEN_CACHE_GRACE` (30 s).
- `src/thumbs.rs:68-84` — `ThumbKey { path, modified, size }`: an edit invalidates the thumbnail.
- `src/thumbs.rs:88-99` — `thumb_format`; `:101-174` header probes reject oversized sources before decoding.
- `src/thumbs.rs:274-329` and `:360-392` — `decode_canvas` / `thumbnail_from_file`, staged `wanted` checks.
- `src/thumbs.rs:414-438` — `render_thumbnail_while`: decode off the UI thread, `.desktop` icon indirection.
- `src/thumbs.rs:475-605` — `ThumbQueue`: `push`/`pop`/`wants`/`release`/`retain`/`clear`, one in-flight key.
- `src/thumbs.rs:610-714` — `Thumbnails`: `entries`, `pending`, `bytes`, `lookup` (`:621`), `begin` (`:636`),
  `start_pass_keeping` (`:657`), `complete` (`:670`), hard-cap guard (`:674-683`), eviction scan (`:688-700`).
- `src/thumbs.rs:732-781` — hidden-window policy (`HiddenCache`, `DroppedThumbs`).
- `src/workers.rs:67-91` — `thumb_worker`: one decoder, staged cancellation, batched wake (`:85-89`).
- `src/main.rs:397` — dedicated low-priority `flamewm-explorer-thumbs` thread.
- `src/app/projection.rs:712` / `:855-866` — pass pruning and row lookup/queue; `src/app/input.rs:501` — `complete`.
- `src/thumbs/tests/mod.rs:341-361` — entry-cap LRU test; `:364-390` — pixel-byte-budget test;
  `:393-410` — new hard-cap test.

## Counted problems and gaps

- Hard byte bound: **1** gap. Before step 1, `complete` inserted first and evicted only while
  `entries.len() > 1` (`src/thumbs.rs:688-700`), so one canvas larger than `CACHE_BYTES` left
  `bytes > CACHE_BYTES`. A real canvas is `THUMB_SIZE * THUMB_SIZE * 4` = 1,296 bytes, so the gap was
  reachable only through the public `complete` API or a future larger thumbnail size.
- Budget tests before this unit: **1** (`cache_evicts_least_recently_used_at_the_pixel_byte_budget`,
  `src/thumbs/tests/mod.rs:364`). It exercised repeated eviction with synthetic 64 KiB chunks but
  never a single oversized payload, so "no decode can exceed the budget" was untested.
- Decode lane: **1** worker (`src/workers.rs:69`, `src/main.rs:397`), already off the UI thread;
  no change was needed here.
- `lookup` advances the LRU clock even on a miss (`src/thumbs.rs:622`): **1** harmless tick loss,
  left as-is so eviction order (and therefore pixels) cannot change.
- Eviction cost: **1** linear `min_by_key` scan per overflowing insert (`src/thumbs.rs:694`), bounded
  by `CACHE_CAPACITY = 256`; worth replacing only if a scoped perf test shows it dominates.

## Reference precedent and choice

- `.vendor/FileWheel-main/src/formats.py:48-52`: a running byte total is checked on each added member
  and the whole extraction is refused once `64 * 1024 * 1024` is exceeded. **Choice:** port the
  hard-cap-at-insertion shape locally — `complete` refuses a canvas larger than `CACHE_BYTES`,
  caching a failure so the row keeps its type icon (the documented fallback at
  `src/thumbs.rs:8-10`). No code was copied verbatim and no dependency was added.
- `.vendor/xfdesktop-master/common/xfdesktop-thumbnailer.c:279-310` and `:376-446`: duplicate requests
  are coalesced with `g_slist_find` and a burst is submitted once by a single 300 ms low-priority
  timer. **Choice:** no code ported — FlameWM already has both properties via `Thumbnails::begin`'s
  `pending` set (`src/thumbs.rs:636`) and the worker's interval wake (`src/workers.rs:85-89`); a
  second debounce would only delay visible rows.

## Target design

Keep the typed `thumbs` facade as the sole owner of cached pixels. Every decode runs on the one
low-priority worker; the UI only reads `Thumbnails`, keyed by path + mtime + size. Admission is a
hard byte budget: a canvas larger than `CACHE_BYTES` is refused and cached as a failure, and the
coldest entries are evicted until both `CACHE_CAPACITY` and `CACHE_BYTES` hold. `HiddenCache` drops
the cache and queue after one grace wake. No pixels are produced on the UI thread and no visible row
changes, so output stays pixel-identical.

## Steps (each compiles alone)

1. **This unit.** Refuse an oversized canvas at insertion in `Thumbnails::complete` and add a
   deterministic test for refusal, exact-budget admission and the real 18 px canvas size. No
   signature or call site changes; UNBUILT here.
2. Export the cached byte count and entry count through the existing profiler snapshot so a scoped
   perf test can rank cache churn against decode cost. Compiles alone with step 1.
3. Replace the linear `min_by_key` scan with an explicit LRU order only if that perf test shows it
   is the largest share of a thumbnail-heavy frame. Compiles alone; visuals unchanged.

## Rollback

Step 1 reverts by restoring `pixels: pixels.map(Arc::new)` in `Thumbnails::complete`
(`src/thumbs.rs:674-683`) and deleting `a_canvas_over_the_byte_budget_is_cached_as_a_failure` from
`src/thumbs/tests/mod.rs:392-410`. No other module imports the guard, so nothing else changes. Steps
2 and 3 revert independently by dropping the profiler field or restoring the linear scan. No new
dependencies are added.

## Step 1 result (UNBUILT)

Production edit: one guard at `src/thumbs.rs:674-683` (a canvas over `CACHE_BYTES` becomes a cached
failure). Test: `src/thumbs/tests/mod.rs:392-410` covers refusal (0 bytes, `Lookup::Failed`),
exact-budget admission and the 1,296-byte real canvas. No build, Cargo, Git or Python command was
run in this unit; all edits are **UNBUILT**.
