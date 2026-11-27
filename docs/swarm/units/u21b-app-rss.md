# u21b — Per-app RSS audit: settings, explorer, desktop at idle

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every MiB
figure below is a cited table value or a code-budget estimate, never a fresh runtime sample.

## Target selection — before any edit

`docs/PERFORMANCE.md:170-174` (single app open, nested Xephyr 1280x800, own process row):

| App | RSS | PSS |
| --- | --- | --- |
| `flamewm-terminal` | 21.3 | 10.3 |
| `flamewm-settings` | 24.5 | 15.0 |
| `flamewm-explorer` | 18.1 | 8.9 |

Terminal is excluded by the brief, shell and task-manager have no row in this table, so the
highest-RSS eligible app is **`flamewm-settings` (24.5 MiB RSS / 15.0 MiB PSS)**. The session-mode
table at `docs/PERFORMANCE.md:144-151` puts desktop at 23.5 MiB RSS and shell at 15.6, which does
not displace settings. Provenance at `docs/PERFORMANCE.md:176-182` warns that these binaries were
not rebuilt for the sample; no new measurement was permitted here.

## Rows — file:line, estimated MiB, change, risk

| file:line | estimated MiB | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-settings/src/theme_picker.rs:298-305` (applied) | ~0.0066 MiB per hidden icon theme (3 × 24×24×4 B example rasters); ~0.10 MiB on a 48-theme host — code budget, not measured | **Applied — drop after use.** After the final icon ordering is fixed, keep only `previews.first()` for themes past `THEME_ROW_SLOTS` (32) and drop their other decoded example rasters. Picker rows 0..31 are untouched, and the selected-value image reads `previews.first()` from any theme, so no visible pixel changes. | Low — read from the actual consumers before editing: `app_impl_2.rs:115-153` uploads only `slot in 0..THEME_ROW_SLOTS` with `items.get(slot)`; `app_impl_3.rs:783-792` uses `selected_preview`, which takes `.first()` (`theme_picker.rs:189-199`); names/labels are unaffected. Requires orchestrator build + visual check; UNBUILT. |
| `crates/flamewm-settings/src/theme_picker.rs:274-301`, `crates/flamewm-settings/src/runtime.rs:54`, `crates/flamewm-settings/src/app_impl_2.rs:28-38` | Visible previews ~0.28 MiB (32 rows × 4 × 24×24×4 B) plus transient scan-thread directories; no steady-state figure measured | None — the whole scan already runs off the UI thread (`start_scan`, `:392-399`), uploads only while the picker is open, and calls `release_free_heap()` once when the result lands (`app_impl_2.rs:34-36`). | n/a — making rasterization lazy until picker-open would show empty rows first (behaviour-changing), so the cut above keeps the scan and trims only never-visible rasters. |
| `crates/flamewm-settings/src/view.rs:8,16,21`, `crates/flamewm-settings/src/runtime.rs:25` | One compiled `flamewm-settings.rwr` decode at launch; on-disk size was not measured in this unit | None — a single decode cannot be deferred without an empty first window; there is no per-page template set to parse. | n/a — audited read-only (`view.rs` is excluded by the earlier u21 ownership split). |
| `crates/flamewm-settings/src/xdg_cache.rs:25-104`, `crates/flamewm-settings/src/app_impl_1.rs:61-69` | Small text projections: one `StartupView` and one defaults label list, <0.01 MiB in a typical XDG tree | None — facade/watch are built on first use and each projection is single-entry and `XdgStamp`-invalidated; the launch `clear()` is required for freshness. | n/a — no unbounded or uncapped document cache in this owner. |
| `crates/flamewm-settings/src/display_layout.rs:82` | One `Vec<OutputPosition>` sized by RandR output count, <0.01 MiB | None — `Vec::with_capacity(active.len())` reserves exactly one entry per active output; this is the only `with_capacity` in the settings crate. | n/a — no oversize reservation found. |
| `crates/flamewm-settings-core/src/xdg_applications.rs:360-377` | Watch-path `Vec`, <0.01 MiB | None — capacity `(config_dirs.len() + data_dirs.len()) * (1 + names)` equals the number of paths actually pushed; built once per facade and reused via `stamp_with`. | n/a — exact reservation. |
| `crates/flamewm-render-core/src/glyphs.rs:73-126` (shared owner, read-only evidence) | Glyph and advance maps capped at `CACHE_LIMIT = 1024` entries each; no steady-state MiB measured | None — the cache is LRU-evicted and lives in `flamewm-render-core`, not in a settings-owned file; settings decodes its document once. | n/a — no uncapped glyph/document cache in scope. |
| `crates/flamewm-settings/src/runtime.rs:22-26`, `crates/flamewm-settings-core/src/lib.rs:192-297` | Authoritative Settings/Displays/Shortcuts/Panels snapshots; unmeasured | None — `refresh_all()` loads every snapshot for first paint; making unseen pages lazy needs a protocol change inside another crate and would change first-paint behaviour. | n/a — recorded as a protocol-level cost, consistent with u21. |

## Verification

UNBUILT by contract: no compiler, test runner, `git` or Python was invoked. The edit was re-read
before and after application; the freshness of the table figures is limited by the binary
provenance note in `docs/PERFORMANCE.md:176-182`. Visual identity is argued from the two call sites
named in row 1, not from a running window. The orchestrator's build, tests and visual check must
confirm compilation and unchanged pixels before any RSS claim is made.

PASS — all four requested startup-allocation categories were checked for the highest-RSS eligible
app from the per-app table: the settings document and XDG projections were already lazy or
single-instance, the only `Vec::with_capacity` reservations were exact, and the glyph cache is
LRU-capped in a shared owner. The one allocation that was genuinely retained after use with no
viewer — example rasters for icon themes beyond the 32-row picker window — was cut by dropping all
but the first preview at the end of the scan, keeping both the visible rows and the
selected-value image pixel-identical. No runtime A/B was permitted, so the slice is marked UNBUILT
and awaits the orchestrator's build and visual verification.
