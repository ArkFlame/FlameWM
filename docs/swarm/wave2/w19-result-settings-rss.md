# w19 — Settings app heap/RSS audit

**Status: UNBUILT.** No `git`, `cargo`, Python, build, test or sleep-loop command was run in this
shared checkout; the orchestrator builds, tests and commits after the batch lands. Every figure
below is a code budget or a value already cited in `docs/PERF_HOTLIST.md` or
`docs/PERFORMANCE.md`, never a fresh runtime sample. Production edits applied: **0** (this is an
audit; the three-edit cap was not needed).

## Route and evidence ledger

- Route: `flamewm-source-first` and `flamewm-reference-mining` loaded. Workspace `AGENTS.md`,
  `scripts/version` (read, never run — it calls Python), `.opencode/HARNESS.md`,
  `.opencode/ROUTES.md` and `.agents/README.md` read. `STATUS: OPEN` in the unit was already
  accompanied by `## Result` / `STARTED`, so that step was not repeated.
- Commands/tools: `rg` on `Cargo.toml` resolved the crate to `crates/flamewm-settings` (plus
  `flamewm-settings-core` and `flamewm-xsettings`); `rg` over both `src/` trees for
  `include_bytes!`, `include_str!`, `with_capacity`, `HashMap`, `BTreeMap`, `OnceCell`,
  `thread_local`, `Box::leak`, `Rc` and `Arc`; `read` of each site named in the rows below;
  `rg`/`read` of `docs/PERF_HOTLIST.md` before ranking, to avoid duplicate rows.
- Reference-first: `.vendor/smithay-master/anvil/src/cursor.rs:9,26-40` loads one cursor theme and
  keeps a single embedded fallback raster rather than a decoded catalog; that is the precedent for
  the settings picker keeping one preview per hidden theme. `.vendor/**` is read-only evidence,
  never a dependency or runtime input. No clone was needed.
- De-dup: hotlist row 10 (`docs/PERF_HOTLIST.md:24`, coalesced settings writes, u134) and row 37
  (`:58`, `p38-settings-app-idle.md`) already cover idle wakeups; row 11 (`:25`) covers embedded
  lock/desktop assets and `w22-result-embedded-assets.md:61` covers the settings RWR embed by disk
  size. This audit ranks settings **heap**, so those rows are not repeated here.

## Rows — file:line, estimated bytes, finding, change, risk

Ranked by estimated retained or transient bytes; "code budget" means no live RSS was sampled.

| file:line | estimated bytes | finding | change | risk |
| --- | ---: | --- | --- | --- |
| `crates/flamewm-settings/src/theme_picker.rs:274-306`, `:276`, `:310-327`; `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:21-41`; `crates/flamewm-settings/src/app_impl_2.rs:28-38` | Scan-thread transient; potentially the largest single budget on a host with many icon themes (one directory-entry map, one `IconLookupIndex` per theme, up to 4 × 24×24×4 B example rasters per icon theme) | The launch scan (`runtime.rs:54`) builds one lookup index per discovered icon theme. Directory reads go through a shared `DirListings` whose `by_dir` `HashMap` has no entry cap, so it grows with every directory visited until `scan()` returns; it is then dropped and the scan thread's free heap is trimmed by `release_free_heap()` when the result lands (`app_impl_2.rs:34-36`). | None. The map is scoped to the scan and already released; `icon_previews` drops each index at return. Deferring the whole scan until the picker opens would change when thumbnails appear. | Medium if made lazy (empty rows until the scan resolves); low for the already-landed retention cut below. No fresh peak sample exists, so no further cut is justified. UNBUILT. |
| `crates/flamewm-settings/src/view.rs:8,15-16` | One compiled document decode (on-disk source `ui/settings.html` is 171,328 B per `w22-result-embedded-assets.md:50`; decoded node heap unmeasured) | `COMPILED_UI` is embedded once and decoded once at launch for first paint; there is no per-page template set to parse. | None — a single decode cannot be deferred without an empty first window. | n/a — out of scope for a zero-visual-change cut. Already listed as an embed in w22; ranked here as heap, not as binary disk. |
| `crates/flamewm-settings/src/runtime.rs:21-26`; `crates/flamewm-settings-core/src/lib.rs:192-297` | Unmeasured; protocol-level | `refresh_all()` loads the authoritative Settings/Displays/Shortcuts/Panels snapshots for first paint. | None — making unseen pages lazy needs a protocol change in another crate and changes first-paint behaviour. | High for any change; recorded as a protocol cost, consistent with `u21-app-rss.md:34` and `u21b-app-rss.md:34`. |
| `crates/flamewm-settings/src/theme_picker.rs:28-38,125-153,189-199,300-304` | ~0.28 MiB for 32 rows × 4 × 24×24×4 B visible icon previews, plus one retained raster per hidden theme after the landed cut | Pixels are shared through `Arc<Vec<u8>>`, so document refreshes hand out handles instead of copying. Only slots `0..THEME_ROW_SLOTS` (32) upload, and the selected-value image reads `previews.first()`. The earlier u21b cut truncates hidden themes to one preview (`theme_picker.rs:300-304`); cursor themes build only one `left_ptr` each, so there is nothing further to trim. | None — visible rows and the selected image are load-bearing and pixel-identical. | Low for the landed cut; no visual change. UNBUILT (not recompiled here). |
| `crates/flamewm-settings/src/xdg_cache.rs:23-105`; `crates/flamewm-settings-core/src/xdg_applications.rs:354-377` | <0.01 MiB in a typical XDG tree | Startup rows, candidate list and default-application labels are memoized per `XdgStamp`; the facade and watched path list are built once. The launch `refresh()` clears the projections for freshness (`app_impl_1.rs:61-69`). | None — already lazy and stamp-invalidated. | n/a — no uncapped document cache in this owner. |
| `crates/flamewm-settings/src/display_layout.rs:82`; `crates/flamewm-settings-core/src/xdg_applications.rs:360-377` | <0.01 MiB | `Vec::with_capacity(active.len())` reserves exactly one entry per active RandR output; `XdgWatch` reserves exactly the number of paths it pushes. These are the only production `with_capacity` sites in the two crates. | None — exact reservations, no growth hazard. | n/a — no change. |
| `crates/flamewm-settings/src/theme_picker.rs:43-48`; `crates/flamewm-integrations-linux/src/icon_theme/lookup.rs:69,139` | Static tables: 4 legacy themes, 4 preview names, 1 bundled theme; memo cap 2048 entries | The constant tables are tiny. `IconLookupIndex::memo` is capped at 2048 names, so repeated lookups do not grow without bound. | None — bounded already. | n/a — no change. |

## Zero-visual-change cut specs (at most two, not applied here)

1. **Landed precedent — hidden-theme preview truncation** (`theme_picker.rs:300-304`). After the
   icon order is fixed, keep only `previews.first()` for themes past row slot 32. Picker rows
   0..31 and `selected_preview()` are untouched, so pixels are identical. Applied by u21b before
   this unit; re-read in current source and confirmed still present. No new edit was made.
2. **Spec only — bound the scan's `DirListings` transient** (`walk.rs:21-41`, called from
   `theme_picker.rs:276,315`). Drop directory entries once the last theme that resolves through
   them has been indexed, or cap the map with a disk-read fallback. This changes no raster and no
   row order, but it needs a unit test in `flamewm-integrations-linux`, which is outside this
   unit's settings-app scope. It is deferred: the map already dies with `scan()`, the scan thread's
   free heap is trimmed on completion, and there is no live peak measurement to justify the
   extra state (the same allocation-gap caveat as hotlist row 15, `PERF_HOTLIST.md:36`).

## Verification

UNBUILT by contract: no compiler, test runner, `git` or Python was invoked. Visual identity is
argued from the two consumers named in row 4 (`app_impl_2.rs:125-153` and `theme_picker.rs:189-199`),
not from a running window. Table figures are code budgets and previously cited table values; the
orchestrator's build, tests and visual check must confirm them before any RSS claim is made.

PASS — all requested categories were checked for the settings app: static tables are tiny,
`include_bytes!` pulls one compiled document that cannot be deferred, the only caches are
stamp-invalidated or entry-capped, both `Vec::with_capacity` sites reserve exact counts, and
per-widget preview buffers are shared through `Arc` with only 32 rows uploaded. The one earlier
zero-visual-change cut is already in the source; the remaining candidate is a transient, already
released scan map whose bounding would need an out-of-scope crate edit and a live peak measurement,
so it is recorded as a spec rather than guessed at. No production edits were made and no runtime
RSS was sampled; the slice is UNBUILT and awaits the orchestrator.
