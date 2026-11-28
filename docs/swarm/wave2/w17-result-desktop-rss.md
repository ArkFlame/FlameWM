# w17 — flamewm-desktop RSS / heap audit

STATUS: PARTIAL

Scope: the `flamewm-desktop` process (the crate plus the caches it instantiates).
Static inspection only: no build, no test run, no RSS sample, no `git`. Every byte
figure below is a **code-budget estimate**, not a measurement. Sites already
ranked in `docs/PERF_HOTLIST.md` are listed under "Not re-ranked" rather than
repeated here.

Reference-first check: `.vendor/xfdesktop-master/src/xfdesktop-icon-view.c:1386-1397`
frees each item's cached pixbuf surface on invalidation instead of accumulating
per-icon surfaces; `.vendor/kwin-master` and `.vendor/smithay-master` had no
desktop icon-raster cache precedent for this path. Semantic finding: upstream
ties raster retention to visible items plus an explicit invalidation event;
FlameWM's generation-scoped `IconLookupIndex` and byte-bounded raster LRU are at
least as bounded. Nothing was copied, included or depended on from `.vendor/`.

## Ranked findings

| rank | site | estimated retained bytes | cause | disposition |
| --- | --- | --- | --- | --- |
| 1 | `crates/flamewm-integrations-linux/src/icons/mod.rs:299-304,684-719`, instantiated at `crates/flamewm-desktop/src/projection.rs:299-305` | 44x44 RGBA = 7,744 B per icon held **twice**; ~0.8-1.5 MB duplicated for 100-200 distinct icons; LRU budget 8 MiB main + 2x512 KiB workers | `cached_or_resolve` seeds the raster LRU with `raster.clone()` and inserts `result.clone()` into the request `HashMap`; `Rgba8Raster.pixels` is a `Vec<u8>`, so both are deep copies of the same pixels | Deferred this unit — cross-owner change and exceeds the two-cut budget; re-rank separately |
| 2 | `crates/flamewm-desktop/src/main.rs:851-855`; `crates/flamewm-ui-document/src/template.rs:769-774`; `crates/flamewm-render-core/src/codec.rs:166-197`; `build.rs:53-71,187-332,347-386` | ~0.5-1.0 MiB decoded node heap; one compiled RWR, ~4k nodes from 128 item slots, 8 widget slots, 576 overlay cells and 18 config rows | `decode_static` borrows ids/text and the 2.5 MB `wallpaper.ppm` pixels from the `'static` RWR; only the node tree, shared styles and layout memo are heap | CLOSED — one decode per process; deferring nodes changes hit-testing |
| 3 | `crates/flamewm-desktop/src/downloads.rs:47,123-132,223-228,235-240` | uncapped `ignored: BTreeSet<String>`; seeded with every Downloads name and grows on failed moves/vanished non-files, cleared only on stop/start | no prune during a running session | CUT 1 spec below |
| 4 | `crates/flamewm-desktop/src/projection.rs:221-248` | 128 slots x 9 owned `String`s = 1,152 strings; ~28 KiB headers + ~28 KiB text ~ 55-60 KiB for process lifetime | eager `OnceLock<Box<[SlotIds; SLOT_COUNT]>>` builds all slot ids even on a sparse desktop | CLOSED this unit — lazy slots would add ~12 KiB of `OnceLock`s plus a hot-path atomic load; exceeds the two-cut budget |
| 5 | `crates/flamewm-desktop/src/widgets/mod.rs:161-175,177-190,245-246` | ~8-12 KiB `sync_key` mirror of widgets/snapshots/four 60-entry `VecDeque`s | `SyncKey` deep-copies inputs so an equal state skips document writes | CLOSED — fingerprint rewrite risks false-equal skipping a repaint |
| 6 | `crates/flamewm-desktop/src/rubber.rs:13-30,42-53,55-61`; `crates/flamewm-desktop/src/state.rs:103` | ~5 KiB transient `Vec<RubberItem>` per gesture, released at gesture end | gesture-local item rect snapshot | CLOSED — already released |
| 7 | `crates/flamewm-desktop/src/icon_raster.rs:3,17-26`; `crates/flamewm-desktop/src/projection.rs:416,542,570` | 7,744 B fresh constant RGBA buffer per cache miss (transient, not retained) | `flame_fallback_raster()` builds a new RGBA8 `Vec` from the constant colour on every miss | CUT 2 spec below |
| 8 | `crates/flamewm-desktop/src/widgets/mod.rs:107-124,251-254`; `crates/flamewm-desktop/src/widgets/sync.rs:316-371` | few KiB of fixed `[Option<..>; 8]` key caches and reused `GraphKey.series` | per-widget rings/faces/analog labels/graph keys | CLOSED — bounded by `MAX_WIDGETS = 8` |
| 9 | `crates/flamewm-desktop/src/widgets/samples.rs:8-14,42-65`; `crates/flamewm-desktop/src/widgets/mod.rs:143,185-189` | ~1 KiB retained plus ~480 B per sample; four `VecDeque`s capped at `HISTORY_LEN = 60` | CPU/memory/network histories | CLOSED — bounded; equality correctness matters more than these bytes |
| 10 | `crates/flamewm-desktop/src/state.rs:27-41,109`; `crates/flamewm-desktop/src/refresh.rs:455-483` | ~0.2-0.4 KiB per `.desktop` launcher; ~20 launchers ~ 4-8 KiB | `BTreeMap<PathBuf, LauncherMeta>`; one entry per launcher, reused on unchanged stamps | CLOSED — bounded by the model's launcher items, needed for label/icon projection |
| 11 | `crates/flamewm-desktop/src/output_layout.rs:196-217` | two `Vec<(OutputId, Cell)>` per item plus cloned output grids: few to tens of KiB | `ProjectedCellsCache` snapshot is the invalidation input | CLOSED — removing it re-captures on every sync |
| 12 | `crates/flamewm-desktop/src/icon_names.rs:27-74`; `crates/flamewm-desktop/src/state.rs:10-25`; `crates/flamewm-desktop/src/ui_contract.rs:5-37`; `crates/flamewm-desktop/src/projection.rs:265-267` | 0 B heap | `const` arrays and `&'static str` tables live in rodata | CLOSED — no heap contribution |

## Not re-ranked (already in `docs/PERF_HOTLIST.md`)

- Retained decoded wallpaper raster ~8-9 MiB at 1080p — row 3.
- Embedded assets: default WebP 234 KB, alternate 640 KB, staged SVGs — row 11.
  The 2.5 MB `assets/raster/wallpaper.ppm` compiled into the RWR
  (`build.rs:12,78-90`, `ui/desktop/index.html:4-5`) is binary size, not heap,
  because `decode_static` borrows its pixels.
- Desktop watcher name set (cap 1024) — row 18.
- Desktop idle wakeups — row B1 (steady-state re-sample still open).
- Desktop "+5.7 MB regression" — rows R2/R5 and the TRUE COMPARISON table
  (line 127: not real).

## Zero-visual-change cut specs (spec only; UNBUILT)

1. **UNBUILT — spec only.** Stat-verified prune of the downloads `ignored` set.
   In `crates/flamewm-desktop/src/downloads.rs:47,123-132,223-228,235-240` the
   set is seeded from the whole Downloads directory and only grows during a
   session. On the existing settle pass, drop names whose path no longer exists
   or is no longer a non-file, keeping entries for files still present so failed
   moves are not retried every tick. No projected pixel or notice changes. Unit
   test beside the code: seed `ignored`, remove the backing file, tick, assert
   the entry is gone while a still-present file stays ignored. Risk: medium —
   prune must be stat-verified so a rename race cannot re-trigger a move; a
   wrong predicate changes file behaviour, not pixels.
2. **UNBUILT — spec only.** `OnceLock` cache of the constant fallback raster.
   `flame_fallback_raster()` (`crates/flamewm-desktop/src/icon_raster.rs:17-26`)
   reallocates 7,744 B on every miss. Return a clone of one `OnceLock`-held
   raster. Every caller already owns its raster (`projection.rs:416,542,570`);
   the only in-place mutation is `fade_ghost_pixels` at line 542, which acts on
   the local clone, so pixels are byte-identical. Unit test beside the code:
   assert two calls return equal bytes, source, size and origin, and that
   fading one clone leaves the cached one unchanged. Risk: low, but do not
   hand out `&mut` access to the shared raster.

## Verification status

No build, test run, screenshot or RSS sample was permitted in this unit. Both
cuts were applied as source edits but are UNBUILT and unverified. Pixel identity
is argued from the code paths (unchanged move decisions for present files;
identical raster bytes), not from a rendered comparison. All byte figures are
code-budget estimates and must be confirmed with a scoped before/after sample
before any cut lands. A `STATUS: OPEN` line did not exist in this unit file at
finalization; `STATUS: PARTIAL` was added in its place.

## Result

STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-desktop/src/downloads.rs:230-234` (test at `:522`) | `ignored: BTreeSet<String>` was seeded from the whole Downloads directory and grew on failed moves and vanished non-files, with no prune during a running session | Added a settle-pass `retain` that keeps only names whose `symlink_metadata(self.dir.join(name))` is `Ok`; present files (baseline and failed moves) stay ignored, vanished names are dropped. Added `vanished_ignored_names_are_pruned_on_settle`, which drains inotify notes before seeding so the prune itself is exercised. No projected pixel or notice changes. | UNBUILT. One stat per ignored name per settle pass (already throttled by `POLL`); a transient metadata error would un-ignore a name and could re-trigger a move. Wrong predicate changes file behaviour, not pixels. No build or test run. |
| `crates/flamewm-desktop/src/icon_raster.rs:18-30` (test at `:36-53`) | `flame_fallback_raster()` rebuilt the constant 7,744 B RGBA buffer on every cache miss | One `static OnceLock<Rgba8Raster>` built on first use; each caller receives `.clone()`, preserving its existing ownership and in-place `fade_ghost_pixels` mutation. Added `fallback_raster_is_cached_and_clones_are_independent` asserting equal bytes/source/size/origin and that fading one clone leaves the cached raster unchanged. | UNBUILT. One atomic load plus a deep clone per miss; no `&mut` access escapes to the shared raster. Pixels are byte-identical for every caller; no build or test run. |

Note: this unit file was concurrently rewritten mid-task. The first `## Result`/`STARTED` append and two edits implementing an earlier version's cut specs (icon-raster de-duplication, lazy slot IDs) were reverted after the authoritative file re-ranked those cuts as deferred/closed; only the two cuts named in the current file remain applied. No `.vendor` code was copied, included or depended on; the recorded xfdesktop precedent (`xfdesktop-icon-view.c:1386-1397`, invalidate-on-event rather than unbounded accumulation) was checked again and remains evidence only.

PARTIAL — both assigned cuts are applied in source with unit tests beside the code, but this unit forbade `git`, `cargo`, builds, tests and RSS sampling, so every edit above is UNBUILT. Pixel identity and the prune predicate are argued from the current code paths, not from a rendered comparison or a green test run; a scoped before/after heap sample and the crate's tests must be run by the orchestrator before either cut counts as landed.
