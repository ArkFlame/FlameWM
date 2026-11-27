# u21 — Terminal and settings app startup RSS audit

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. All costs
below are code budgets or figures already cited in the hotlist, not fresh runtime samples.

**Scope:** `crates/flamewm-settings/src` excluding `nav.rs`, `actions.rs`, `consts.rs` and `view.rs`
(owned by the AI-usage settings-page worker), and `crates/flamewm-terminal/src`. Hotlist row 2 is
`docs/PERF_HOTLIST.md:16`; `rg -il 'pool tier' docs` resolves to `docs/PERF_HOTLIST.md`,
`docs/swarm/units/u07-ram-audit.md`, `docs/PERFORMANCE.md` and `docs/perf-loop/LOG.md`.

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-terminal/src/app.rs:190` | `App::new` parsed all 20 built-in theme rows (~400 `from_str_radix` calls, 20 `Vec<u32>` and 20 `String` allocations) only to seed `palette`, which `reload_themes()` overwrites at `:219` before anything reads it. Wasted launch-time transient heap. | Seed `palette` from the new `themes::DEFAULT_PALETTE` const; the dead parse is gone. | Low — identical Dark+ literal values. `refresh_metrics()`, `set_window()` and `relayout()` between the field init and `reload_themes()` do not read `palette`. |
| `crates/flamewm-terminal/src/themes.rs:43`, `crates/flamewm-terminal/src/theme.rs:62,73,79` | `follow_palette` and both `resolve_palette` fallbacks re-parsed the whole built-in table to read row 0 on each resolve, adding another transient Vec/String set per theme change and on first-run default resolution. | Added `DEFAULT_PALETTE` as a const with the Dark+ values and used it for those fallbacks; `builtin_themes()` is now called only where the full named list is actually needed. | Low — same byte values as `BUILTIN[0]`. `theme.rs:174-176` still asserts an unknown name falls back to `themes[0].palette`, so a value drift is caught by the existing test. |
| `crates/flamewm-terminal/src/main.rs:30,179,260`; `crates/flamewm-terminal/src/pool.rs:14-25` | Three pre-baked documents on disk (~0.7 + 2.3 + 6.9 MB of the ~13.3–14 MB binary); decoding the fitted tier costs 2/9/23 ms and +1.35/+4.2/+12.2 MB (tiers 0/1/2). | None — only the tier the grid fits is decoded at launch (`:179`) and a resize decodes the replacement tier (`:260`); unused tiers stay file-backed. This is the already-landed half of hotlist row 2. | No behaviour-preserving change is available: per-slot lazy run-node allocation needs the live slot-occupancy RSS measurement named in row 2, so it is deferred rather than guessed. |
| `crates/flamewm-terminal/src/app.rs:216`; `crates/flamewm-terminal/src/suggest.rs:20-22,476-502` | `History::open_default()` reads the shared command store at launch and builds the prefix trie; reads are bounded (`COMPACT_BYTES` 1 MiB, first-run seed cap 2 MiB) but resident for the window's life. | None — suggestions are consulted on the first key press (`app/keyboard.rs:114`), so the trie is needed immediately; no parse-on-first-use path exists without new interior mutability. | Medium — deferring the trie would change first-keystroke latency and suggestion availability, so it is not behaviour-identical. |
| `crates/flamewm-terminal/src/store.rs:339-343` | `all_themes()` parses built-ins plus user TOML once at launch through `reload_themes()` (`app.rs:235-238`). | None — the named list is load-bearing for theme resolution and the settings view; the redundant row-0 parses that surrounded it were removed in the two rows above. | Low — no change to this call. |
| `crates/flamewm-settings/src/view.rs:8,16,21` (read-only evidence; file excluded from edits) | One pre-compiled `settings.rwr` is decoded once at launch for first paint. There is no per-page template set to parse, so the "parse every template" pattern does not apply here. | None — a single decode cannot be deferred without an empty first window. | n/a — excluded file, no edit made. |
| `crates/flamewm-settings/src/runtime.rs:54`; `crates/flamewm-settings/src/theme_picker.rs:270-300`; `crates/flamewm-settings/src/app_impl_2.rs:17-36,93-146` | The launch-time theme scan builds one `IconLookupIndex` per discovered theme and rasterizes up to four 24×24 RGBA previews (~2.3 KB each) per icon theme plus one `left_ptr` per cursor theme. Only the first `THEME_ROW_SLOTS` (32) rows ever upload, and previews upload only while the picker is open; the scan thread's free heap is trimmed when its result lands. | None — work is already off the UI thread, bounded by the picker slot count and trimmed on completion. Deferring rasterization to picker-open would change when thumbnails appear. | High — a lazy preview path needs a new request channel and would show empty rows until it resolves; deferred as behaviour-changing. |
| `crates/flamewm-settings/src/xdg_cache.rs:25-104`; `crates/flamewm-settings/src/app_impl_2.rs:766-770` | XDG facade/watch are built on first use, and startup rows and default-application labels are memoized per `XdgStamp`; both pages project only while visible, so a repaint of another page does no file reads. | None — already lazy and stamp-invalidated; the launch `refresh()` clearing the cache (`app_impl_1.rs:63`) is required for freshness. | n/a — no change. |
| `crates/flamewm-settings/src/display_layout.rs:82` | `Vec::with_capacity(active.len())` is sized by the active RandR output count (a handful), not a large reservation. | None — bounded by topology. | n/a — no change. |

## Verification

UNBUILT by contract: no compiler, test runner, `git` or Python was invoked. The const values in
`themes.rs` were transcribed from the `BUILTIN` row-0 hex list and checked against the original
string by inspection only; the existing `theme.rs` fallback test is the guard for a value drift,
but it was not executed here.

PASS — all four eager-startup categories were audited across both in-scope crates: the terminal's
tier documents, the settings document and XDG pages were already lazy, the theme preview scan was
already off-thread, trimmed and only uploaded while its picker is open, and the only large
reservation found was bounded by RandR output count. The one genuinely redundant eager parse —
parsing all 20 built-in theme rows to obtain the row-0 fallback palette at launch and on every
fallback resolve — was made lazy through a `DEFAULT_PALETTE` const with identical literal values;
the remaining candidates were left unchanged because deferring them would alter first-paint or
first-keystroke behaviour or needs the live slot-occupancy measurement that hotlist row 2 requires.
The slice is marked UNBUILT and awaits the orchestrator's build and tests.
