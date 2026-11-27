# u27 — Icon cache and catalog memory cut

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Costs below
are code budgets and explicit slot-count bounds, not fresh runtime samples. Rasters and their
source strings are unchanged, so the cut is pixel identical by construction.

**Hotlist row 4:** `docs/PERF_HOTLIST.md:18` — "Shell icon raster cache + catalog shadow", bounded
LRU 2048 entries / 32 MiB. The duplicated `Vec<DesktopApplication>` catalog shadow was already
replaced by the compact `CatalogEntry { id, name, icon_name }` fallback in u75
(`docs/perf-loop/LOG-u75-ram-structs.md:9-19,57-73`), so no catalog file was owned or edited here.

## Owned files (listed before any edit)

- `crates/flamewm-shell/src/async_projection/icon_cache.rs` — the LRU raster cache, `IconRaster`,
  the entry/byte budgets and their tests.
- `crates/flamewm-shell/src/async_projection/icon_binding.rs` — exact binding and the paint boundary
  that consumes a cached `IconRaster`.
- `crates/flamewm-shell/src/app/icons.rs` — worker-result drain that feeds the cache.

`rg -n 'icon' crates/flamewm-shell/src -l | head` was used to locate these. Files under
`projection/start.rs` and `projection/usage.rs` were **not** touched; no file outside the three
above was edited.

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/async_projection/icon_cache.rs:7,340-343` | 2048-entry default retained rasters for names that no visible node could show after an epoch bump, a category switch or a window close. | Lowered `ICON_CACHE_ENTRY_BUDGET` from 2048 to 512 and updated the bound assertion. Eviction already is recency based (`get` refreshes the stamp at `:115-123`; `insert` pops the oldest live record at `:134-141`), and every visible slot re-calls `cached_image` on each projection, so the whole live set stays at the front. Visible-key ceiling is far below 512 (8 Start + 8 launcher + 8 task/output + 6 tray/output + 24 dock, each at its own drawn edge), leaving headroom for a few stale generations. | Medium until measured: the "no visible icon evicted in steady state" gate is argued from the slot counts and the recency touch on every projection, not from a runtime sample; builds/tests are forbidden here, so the cap is marked UNBUILT for the orchestrator to confirm. |
| `crates/flamewm-shell/src/async_projection/icon_cache.rs:17-32,143-156`; `crates/flamewm-shell/src/async_projection/icon_binding.rs:143-155` | Each `IconRaster` carried its own `String source`, a second owned copy of the name already held as `Arc<str>` by the cache key; every `IconRaster::clone` (cache hit, per-output panel paint, stale-bind repaint) allocated it again. | Changed `IconRaster.source` to `Arc<str>`, converted the incoming `RuntimeImage` name with `Arc::from`, and assigned the cache key's `Arc` into the raster on insert so key and raster share one name allocation. The paint boundary converts back to an owned `String` only where the document trait requires it. | Low — identical source bytes and pixel data; `Arc<str>` derefs to `str` for the loader's `ends_with` checks and the test comparisons now compare `as_ref()`. The paint conversion is one allocation at the document boundary, no more than the old clone cost. |
| `crates/flamewm-shell/src/app/icons.rs:80-85` | The cache key used the resolved raster's own dimensions, not the edge the node is drawn at. A resolver that handed back a larger source raster would retain it under a larger key that no drawn-size lookup can hit, spending byte budget without ever serving a hit. | Feed `note_icon_result` with `res.width`/`res.height` (the requested/drawn edge) instead of `raster.width`/`raster.height`. The raster itself, and the node paint, are untouched. | Low — the resolver scales every raster to the requested physical edge (`flamewm-integrations-linux/src/icons/mod.rs:1055-1072,1091,1123-1138`), so keys are unchanged in the normal path; any mismatch now stores at the drawn size rather than a larger one. |
| `crates/flamewm-shell/src/async_projection/icon_cache.rs:9-10,126-133`; `crates/flamewm-shell/src/async_projection/icon_cache.rs:43-56,159-172` | The 32 MiB byte budget, the oversize-raster rejection and the lazy recency/compaction machinery already bound memory and evict by recency. | None — kept as is. These are the authority for the byte cap and LRU order; changing them would add slower icon loads or visual churn for no measured gain. | n/a — no change. |
| Catalog shadow (`docs/PERF_HOTLIST.md:18`, u75) | Previously a second full `Vec<DesktopApplication>` retained every catalog string. | None — already compacted in u75; no owned file. | n/a — out of scope, no edit. |

## Verification

UNBUILT by contract: no compiler, test runner, `git`, Python or sleep loop was invoked. Pixel
identity is argued from source only: no raster dimension, pixel byte, source string value or node
id changes, and the loader already rasterizes at the requested physical edge. The lowered entry
cap's steady-state no-eviction condition is bounded from the slot counts above but needs the
orchestrator's build, tests and a scoped icon-cache sample to confirm.

PARTIAL — all three requested tightenings were applied: recency eviction now runs at a 512-entry
default with the live slot set argued to stay resident, the cache stores under the drawn size
instead of a possibly larger source-raster size, and the duplicate per-raster `String` name is
shared as an `Arc<str>` with the cache key. The catalog shadow was already compacted in u75 and
needed no edit. The slice is marked UNBUILT and awaits the orchestrator's build, tests and a
runtime check that no visible icon is evicted at the lower cap.
