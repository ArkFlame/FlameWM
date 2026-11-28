# w44: Icon-theme pipeline remaster — design

Status: step 1 implemented, UNBUILT (no `cargo`, no build, no visual change).
Scope: `flamewm-integrations-linux` icon lookup, `icon_service.rs`, shell loader.
Constraints: no new dependency, no pixel change, no widening into other crates'
ownership, no runtime/perf claim without fresh evidence.

## Current structure (file:line)

| Owner | State | Location |
| --- | --- | --- |
| Shell raster LRU | `IconCache`, 512 entries / 32 MiB, lazy recency, global `OnceLock<Mutex<..>>` | `crates/flamewm-shell/src/async_projection/icon_cache.rs:8`, `:11`, `:51`, `:180-183` |
| Shell raster hits/misses | `cached_image`, `note_icon_result`, `invalidate_icon_cache` | `icon_cache.rs:185-210`; callers `icon_binding.rs:177`, `app/icons.rs:82`, `app/settings.rs:31` |
| Lookup `exists` cache | `IconThemeCaches`, `EXISTS_CAP = 4096`, arbitrary first-key eviction | `crates/flamewm-integrations-linux/src/icon_theme/caches.rs:11`, `:19`, `:44-46`, `:54` |
| Lookup name memo | `NameMemo`, arbitrary first-key eviction | `caches.rs:131`, `:141-162` |
| Resolver use of both caches | `ThemeLookupCaches` alias; FS work only on miss | `crates/flamewm-integrations-linux/src/icons/mod.rs:14`, `:4-5` |
| Service result cache | `RESULT_CACHE_ENTRIES = 2048`, `RESULT_CACHE_BYTES = 2 MiB`, `HashMap` first-key eviction | `crates/flamewm-integrations-linux/src/icon_service.rs:149-150`, `:520-529`, `:537-540` |
| Service shared index / workers | one `Arc<IconLookupIndex>` behind `Mutex`, 20 workers with private caches | `icon_service.rs:3-8`, `:138`, `:145` |
| Theme swap | `install_theme_index`, generation epoch; workers adopt before resolve | `icon_service.rs:352-373`, `:65-78` |
| Shell loader index | one worker owns `IconResolver`; rebuild queued, applied on tick | `crates/flamewm-shell/src/icon_loader.rs:1`, `:20-28` |
| Desktop index | own `IconResolver::from_environment`, own 2-worker service | `crates/flamewm-desktop/src/projection.rs:301-305` |
| Explorer index | own `IconResolver::from_environment_with_theme` | `crates/flamewm-explorer/src/mime.rs:256` |
| X11 render image cache | adjacent native raster cache (not theme-index state) | `crates/flamewm-render-x11/src/app/images.rs:341-366` |

## Problems (statically counted, not measured)

These are counts of independent owners and code paths from the sources above.
They are **not** timing, RSS, or hit-rate measurements; no perf conclusion is
claimed here (see `flamewm-performance-proof`).

1. **Six independent cache owners.** Shell raster LRU, theme `exists`, name
   memo, service result cache, per-worker private caches, and the X11 render
   image cache each keep their own entries and eviction rule (one LRU, five
   arbitrary/hash-order policies). Only one is a real LRU.
2. **Three independent theme indexes per session.** Shell (`icon_loader.rs:1`,
   `:20-28`), desktop (`projection.rs:301`), and explorer (`mime.rs:256`) each
   construct an `IconResolver` from the environment. Their directory walks and
   `index.theme` inheritance state are not shared; each process holds a copy.
3. **Eager vs lazy split.** Desktop and explorer build an environment resolver
   at service spawn / dialog open (`projection.rs:299-304`, `mime.rs:251-256`);
   the shell rebuilds on theme change (`icon_loader.rs:20-28`). Lookup itself is
   lazy — filesystem work happens only on a miss (`icons/mod.rs:4-5`) — but a
   full index build is eager and repeated per process.
4. **Eviction is not recency-based where hit rate matters.** `exists` and the
   service result cache evict `keys().next()` (`caches.rs:54-69`,
   `icon_service.rs:524-528`), and `NameMemo` evicts `inner.keys().next()`
   (`caches.rs:151-162`). Those keys are hash-ordered, not least-recently-used,
   so a warm icon can be dropped while a cold one is retained.

### Reference precedent

`.vendor/xfdesktop-master/src/xfdesktop-icon-view.c:2220` obtains one
screen-scoped theme with `gtk_icon_theme_get_for_screen`, and `:2702`,
`:2830-2850` connect/disconnect a theme-changed signal. Semantic finding: one
shared, screen-scoped theme index with change notification, rather than one
index per icon view. This supports a single generation-scoped index owner in
FlameWM; it does not override the current typed ownership boundary. No relevant
KDE `index.theme` cache source exists in `.vendor/**`; the corpus is
incomplete for KDE, so no KDE precedent is claimed.

## Target design

One typed owner in `flamewm-integrations-linux` exposes a shared,
generation-scoped index facade. All consumers resolve through it. A single
bounded-LRU policy primitive backs the lookup `exists` cache, the name memo,
and the service result cache; the shell raster cache may adopt the same
primitive while keeping its byte budget. The index image is stored once in a
generation-stamped file and mapped read-only; a generation bump swaps the
mapping atomically and leaves old entries unreachable (no explicit clear of
live raster bytes). No UI or WM authority moves.

## Steps (each compiles and passes its own tests before the next)

1. **Add the pure `BoundedLru<K, V>` primitive and its tests (this unit).**
   New `icon_theme/lru.rs` + one module line in `icon_theme.rs`. No call site
   changed, no behavior or pixel changed. UNBUILT here; orchestrator verifies.
2. Move `IconThemeCaches::exists` to `HashMap<u64, BoundedLru<PathBuf, bool>>`;
   keep generation scoping and `stats`/`memory_estimate_bytes`. Compile, test.
3. Move `NameMemo` to a `BoundedLru` keyed by `(String, u32)`; add a borrowed
   lookup if the hot path needs it. Compile, test, compare hit counts.
4. Move `ResultCache` to a `BoundedLru<Arc<IconKey>, ..>` with a byte-accounting
   wrapper (the value cost is computed as today). Compile, test.
5. Re-express the shell raster cache on the same primitive, preserving its
   512/32 MiB budgets and `Arc` pixel sharing. Compile, test, compare pixels.
6. Add one generation-stamped, read-only mapped index file owned by the Linux
   integration facade; switch shell, desktop, and explorer off
   `from_environment*` builds. Requires a documented `unsafe` boundary and an
   explicit dependency/API justification; compile, test.
7. Add scoped cache metrics and prove the change with a repeatable baseline,
   metric, threshold, and retained evidence per `flamewm-performance-proof`.
   Reject any step that regresses correctness, bounds, or responsiveness.

## Step 1 implementation and plug-in points

`BoundedLru` lives at
`crates/flamewm-integrations-linux/src/icon_theme/lru.rs`; it is exported
through `icon_theme.rs:16-19`. It provides `new`, `get` (refreshes recency),
`peek` / `contains` (do not), `insert` (replaces or evicts LRU), `remove`,
`len`, `is_empty`, `capacity`, and `clear`. Lazy recency records are compacted
once they outnumber live entries. No current call site uses it, so step 1 is
behavior-neutral.

It plugs into step 2 at `caches.rs:11-52` (inner `exists` map + `evict_one_exists`),
step 3 at `caches.rs:131-168` (`NameMemo`), and step 4 at
`icon_service.rs:517-540` (`ResultCache::finish` eviction loop). A byte-budget
cache such as `icon_service.rs:150` or `icon_cache.rs:11` needs a thin wrapper
that tracks bytes; `BoundedLru` deliberately owns only entry count and recency.

## Rollback

Every step is an isolated diff with no on-disk format change. Step 1 rollback:
delete `icon_theme/lru.rs` and remove the two `mod lru; pub use lru::*;` lines
from `icon_theme.rs`. Nothing else references the type, so no other file or
state needs reverting. Later steps revert only their own file/hunk; the
generation-stamped index file (step 6) is additive and can be ignored by an
older binary.

## Verification limits

No `cargo`, build, test, or runtime command was run for this unit (worker brief
forbids them). All edits are UNBUILT. Visual output is unchanged because no
production call path consumes `BoundedLru` yet; this is a structural claim from
source inspection, not a rendered-image comparison.
