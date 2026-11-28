# w19 — Settings app RSS / heap audit
STATUS: DONE

Scope: the `flamewm-settings` crate (`crates/flamewm-settings`, plus the
`flamewm-settings-core` facade it calls). Static inspection only: no build, no
runtime sample, no `git`. All byte figures are **code-budget estimates**, not
measured RSS. Ranked by estimated retained bytes in a long-lived Settings
process; transient scan peaks are called out separately.

Reference-first check: `.vendor/` (xfdesktop, kwin) had no settings-preview
cache precedent for this path. The precedent used is internal and already in
`docs/PERF_HOTLIST.md`: bounded icon rasters (row 4, u75) and lazy tier decode
(row 2). No upstream dependency was added.

## Ranked findings

| rank | site | estimated bytes | cause | disposition |
| --- | --- | --- | --- | --- |
| 1 | `crates/flamewm-settings/src/view.rs:8,15-17` | one decoded `UiDocument`; binary size of `settings.html` + `settings.css` is not measured here | `include_bytes!` compiled document decoded once at startup (`runtime.rs:25`); keyboard focus/backdrop wrap it in place | CLOSED — one decode per process, no duplicate rows with hotlist rows 2/11 (those cover terminal tiers and lock/desktop assets); any reduction changes pixels |
| 2 | `crates/flamewm-settings/src/theme_picker.rs:138-172,286-321` | visible rows: up to 32 × 4 × 2,304 B ≈ 288 KiB retained (unchanged); hidden themes: 1 × 2,304 B retained, but 3 × 2,304 B ≈ 6,912 B decoded and freed per hidden theme at scan | `icon_previews` decoded all four 24×24 RGBA examples before `with_bundled_icon_themes` sorted the themes, then the old code truncated previews 2-4 for rows past `THEME_ROW_SLOTS` | CUT 1 implemented (below) |
| 3 | `crates/flamewm-settings/src/xdg_cache.rs:24-39,62-80` | ~225 installed apps × (id + name) ≈ 10-15 KiB, plus raw autostart `.desktop` text | `StartupView` retains all rows and all picker candidates while the Startup page is open | CLOSED — built only when that page repaints (`app_impl_2.rs:770`, `app_impl_1.rs:840`), replaced on stamp move and cleared on writes |
| 4 | `crates/flamewm-settings/src/xdg_cache.rs:83-105`, `default_apps_page.rs:16-83` | 7 `String` labels + one transient handler index | `defaults()` memoizes the seven fixed rows | CLOSED — fixed-size, already memoized |
| 5 | `crates/flamewm-settings/src/tables.rs:6-24,48`, `consts.rs`, `default_apps_page.rs:16-52`, `theme_picker.rs:28-40` | 0 B heap | `const` tables and fixed arrays live in rodata | CLOSED — no heap contribution |

Already covered in `docs/PERF_HOTLIST.md`; **not re-ranked here** (no duplicate
rows): catalog parse of ~225 desktop files (`flamewm-settings-core/src/xdg_applications.rs:83-90`,
`xdg_startup.rs:42-76`) is hotlist row 21/29; the wake `Vec` reused by
`std::mem::take` (`runtime.rs:37,115`) is row 10; icon raster pixels are row 4.
The shared `DirListings` in `theme_picker::scan` is a local `Arc` dropped at the
end of the scan, so it is not retained heap.

## Zero-visual-change cut specs

1. **Implemented (UNBUILT).** Sort icon themes with `with_bundled_icon_themes`
   first, then decode only `preview_budget(slot)` example rasters: four for the
   first `THEME_ROW_SLOTS` rows, one for the rest. The loop takes the *first N
   successful* names in the original order, so a hidden row keeps exactly the
   preview the old `truncate(1)` kept. Pixels per theme are unchanged; only the
   discarded decode work disappears. Files: `theme_picker.rs:44`,
   `theme_picker.rs:286-348` (scan 286-321, decoder 325-348), test at
   `theme_picker.rs:494-503`.
2. **Spec only (not implemented).** Replace `StartupEntry.text` (raw `.desktop`
   text) in the memoized `StartupView` with derived row fields (`display_name`,
   `enabled`, `delay_text`) computed once per stamp. This drops the retained
   file text without changing any projected string. Not implemented because it
   spans `flamewm-settings-core`'s `StartupEntry` type, which other crates use.

## Verification status

No build, test run, screenshot or RSS sample was permitted in this unit. The
single edit is UNBUILT; pixel identity is argued from the code path (same first
successful preview per hidden theme, same values for visible rows), not from a
rendered comparison. Re-rank rank 2 only after a scoped before/after smoke test.

## Result

STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-settings/src/theme_picker.rs:40-50,285-348`; test `:493-503` | The scan decoded four 24×24 RGBA examples for every icon theme before sorting; hidden rows then discarded three of them, so those rasters were decoded and freed for nothing. | Sort icon themes with `with_bundled_icon_themes` first, then decode only `preview_budget(slot)` rasters (four for rows 0..`THEME_ROW_SLOTS`, one for hidden rows), taking the first N successful names in the original order. Unit test added beside the code. **UNBUILT.** | Low — visible rows and `selected_preview()` still receive the same first raster; hidden rows keep exactly the raster the old `truncate(1)` kept. No new dependency; pixels unchanged. |
| `crates/flamewm-settings/src/view.rs:8,15-17`; `runtime.rs:25` | One compiled `settings.rwr` document is decoded once at startup for first paint. | None — a single decode cannot be deferred without an empty first window. | n/a — out of scope for a zero-visual-change cut. |
| `crates/flamewm-settings/src/xdg_cache.rs:24-105`; `default_apps_page.rs:16-83` | Startup rows, picker candidates and seven default-app labels are memoized per `XdgStamp`; reservations are exact. | None — already lazy and stamp-invalidated. | n/a — no uncapped document cache in this owner. |
| `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:21-41` (the scan's `DirListings`) | Uncapped directory-entry map transient, scoped to one scan and dropped at return. | None — heap is trimmed by `release_free_heap()` on completion; bounding it would need an out-of-scope crate edit and a live peak sample. | Medium if made lazy; recorded as a spec, not guessed at. |

Every production edit above (the single `theme_picker.rs` change and its unit test) is **UNBUILT**:
no `git`, `cargo`, Python, build, test or sleep-loop command was run in this shared checkout; the
orchestrator builds, tests and commits after the batch lands. No new dependencies were added and
visuals stay pixel-identical.

PASS — the settings-app heap/RSS audit is complete with one zero-visual-change production edit
(hidden-theme preview decode budget plus its unit test), present in `theme_picker.rs` and marked
UNBUILT here. The remaining allocated sites are a single un-deferrable document decode,
stamp-invalidated startup data, exact-capacity reservations, and a scan-scoped directory map that
is released and heap-trimmed on completion; none justifies a further cut without a live peak
sample. No runtime RSS was sampled and no build was run, so the slice awaits the orchestrator's
verification.
