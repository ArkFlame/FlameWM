# App scout: FileWheel, Kensaku, Terminator, RustFetch, Ark vs FlameWM explorer, terminal, flamefetch

Snapshot: 2026-11-09, branch `main`. Read-only scout. No source edits, no cargo builds, no commits.
Constraint applied: no glass, blur, shadow or glow additions. Only reshaping existing elements.

## Premise corrections

- FileWheel is a GTK4 file-conversion wheel (`.vendor/FileWheel-main/README.md`), not a file manager. Only its keyboard and feedback ideas are in scope.
- Ark has no `src/` at `.vendor/ark-master/` top level (only `autotests/`, `po/`, `LICENSES/`). Its conflict and progress code was not read. Rows marked UNVERIFIED depend on it.
- Licences: FileWheel MIT (`LICENSE`), Kensaku MIT (`LICENSE`), RustFetch MIT (`LICENSE`), Terminator GPL-2.0 (`COPYING`), Ark unverified. Reimplement ideas only. Copy no code.

## Scope limits

- Static reads and grep only. No runtime evidence. Every "gap" below is a source-level claim.
- Kensaku UI read through `src/App.tsx` grep only. `SearchBar.tsx` not read.
- FileWheel `wheel.rs` read by grep only.
- Terminal overlay UI rendering not read. Items that depend on it are marked UNVERIFIED.

## Candidate table (23 rows)

Effort: S < 1 day, M 1 to 3 days, L > 3 days. Perf/mem: none unless stated.

| # | Idea | Source path:line | FlameWM today (path:line) | Gap | Effort | Perf/mem | Licence | Verdict and reason |
|---|---|---|---|---|---|---|---|---|
| 1 | Search match-case toggle and wrap toggle | `.vendor/terminator-master/terminatorlib/searchbar.py:123-136` | `crates/flamewm-terminal/src/search.rs:31` `fold()` always case-folds. `search.rs:98` `step()` already wraps. `app/overlay.rs` has no case option. | No case-sensitive mode | S | none | GPL-2.0, reimplement | ADOPT. Small, user-visible, no cost. |
| 2 | Ranked name search (exact > prefix > substring) | `.vendor/kensaku-master/src-tauri/src/search/mod.rs:1072-1075` (fuzzy tier before BM25) | `crates/flamewm-explorer/src/search.rs:42-45` `name_matches` is plain `contains`. Hits return in walk order. | No ranking. Results depend on directory walk order. | S-M | Sorts at most `MAX_HITS=2000` (`search.rs:17`). Negligible. | MIT, reimplement | ADAPT. Adopt tiers only. Skip subsequence fuzzy to stay lean. |
| 3 | Show "results truncated" notice | `search.rs:36` `truncated` flag already produced | UI consumption not verified (`app/` not read). `MAX_VISITS=200000` (`search.rs:18`) can silently cut results. | Possible silent truncation | S | none | n/a | ADAPT, UNVERIFIED UI. Show a count line when `truncated`. |
| 4 | Keyboard pane resize | `.vendor/terminator-master/terminatorlib/paned.py:333-364` `resizeterm` | `crates/flamewm-terminal/src/app/layout.rs:376` `split_focused`, `:412` `close_pane`. No resize key path in `app/keyboard.rs` (grep empty). Pane sizes only change via relayout (`layout.rs:299`). | No keyboard resize of splits | M | none | GPL-2.0, reimplement | ADAPT. Clamp with existing `split_fits` (`layout.rs:279`). |
| 5 | Re-render flamefetch after resize before shell | `.vendor/rustfetch-main/README.md`, `--install-hook` section | `crates/flamewm-terminal/src/fetch.rs:161` `exec_shell` replaces the process at once. `fetch.rs:407` `terminal_dimensions` runs once. No SIGWINCH path. | Output is not redrawn if the window is resized before the first command | M | none | MIT, reimplement | ADAPT, low win. |
| 6 | Pane rotation (swap split orientation) | `.vendor/terminator-master/terminatorlib/paned.py:429` `rotate_recursive` | `layout.rs` has no rotate. `screen.rs:321-329` is scroll-region rotate, not pane rotate. | No pane rotate | M | none | GPL-2.0 | ADAPT later. Win is small. Do after rows 1-4. |
| 7 | Broadcast input to several panes | `.vendor/terminator-master/terminatorlib/borg.py`, README "group terminals" | No `broadcast` in `crates/flamewm-terminal/src` (grep empty). | No group input | M | none | GPL-2.0 | REJECT for now. Accidental multi-shell keystrokes are a safety risk. |
| 8 | Saved and launchable layouts | `.vendor/terminator-master/terminatorlib/layoutlauncher.py` | None. `settings_cache.rs:15` caches settings only. | No layout persistence | L | disk only | GPL-2.0 | REJECT for now. Needs a config writer and a new surface. |
| 9 | Kensaku on-disk metadata index | `.vendor/kensaku-master/src-tauri/src/indexer/metadata_db.rs` | Live walk search, `search.rs:55-170`, no index. | Index absent | L | Index RAM and write churn. Kensaku claims ~12 MB for 100k names. Not measured here. | MIT | REJECT. Violates lazy-first. Live walk is already bounded. |
| 10 | Kensaku memory release on hide | `.vendor/kensaku-master/src-tauri/src/util/memory.rs:9-51` | Thumb cache is bounded at 256 KiB (`crates/flamewm-explorer/src/thumbs.rs:30-32`). | Little to reclaim | S | Negligible | MIT | REJECT. Already bounded. |
| 11 | Bounded producer channel for indexing | `.vendor/kensaku-master/src-tauri/src/indexer/mod.rs:104` | Progress throttle `progress.rs:14` `MIN_INTERVAL = 100ms`. Thumb queue bounded (`thumbs.rs:324-333`). | Already present | none | none | MIT | REJECT. Duplicate. |
| 12 | Bounded LRU for thumbnails | Kensaku analog | `crates/flamewm-explorer/src/thumbs.rs:30-32` (`CACHE_CAPACITY=256`, `CACHE_BYTES=256KiB`), `:367` LRU struct | Already present | none | none | n/a | REJECT. Duplicate. |
| 13 | Type-ahead in listing | Ark / FileWheel analog | `crates/flamewm-explorer/src/view.rs:100` `typeahead_match` with wrap test `:343` | Already present | none | none | n/a | REJECT. Duplicate. |
| 14 | Overwrite / skip / keep-both conflict policy | Ark (UNVERIFIED, `src/` not located) | `crates/flamewm-explorer/src/progress.rs:405` `Conflict::KeepBoth` | Overwrite/skip UI not verified | S (if needed) | none | GPL/LGPL (unverified) | UNVERIFIED. Check the explorer conflict dialog before deciding. |
| 15 | Zero-fork hardware reads | RustFetch README "Zero-Fork Kernel Telemetry" | `fetch.rs:245` `read_small` reads `/proc` and `/sys`. No `Command::new` in `fetch.rs` (grep empty). | Already present | none | none | MIT | REJECT. Duplicate. |
| 16 | Kitty / Sixel logo rendering | RustFetch README | `fetch.rs:463` `kitty_graphics_supported`, `:487` `kitty_image` | Already present | none | none | MIT | REJECT. Duplicate. |
| 17 | Live CPU/RAM sparkline monitor (`--live`) | RustFetch README | None | Feature absent | M-L | Continuous wake-ups. Costs CPU and RAM while open. | MIT | REJECT. Not asked. Violates idle-cost goals. |
| 18 | TUI config builder with `.bak` backups | RustFetch README `--setup` | No config writer in `fetch.rs`. `Config::load` only (`fetch.rs:80`). | Feature absent | L | none | MIT | REJECT. Adds a writer surface. Reliability pattern (atomic write, `.bak`) noted for later. |
| 19 | Self-update with release check | RustFetch README `--self-update` | None | Feature absent | M | Network call from a fetch tool | MIT | REJECT. Network and privacy cost. |
| 20 | Hover glow on buttons and segments | FileWheel `src/style.css:13-15`, `:15` `effect-preview-active` | n/a | User forbids glow and shadow | n/a | n/a | MIT | REJECT. User constraint. |
| 21 | Modifier-key mode select (Shift/Ctrl/Alt) and arrow segment select | FileWheel README "Use Convert, Tools, or Actions" | Explorer keyboard in `crates/flamewm-explorer/src/app/input.rs:330` `step_selection`. No wheel in FlameWM. | Not applicable to FlameWM surfaces | n/a | n/a | MIT | REJECT. Out of scope. |
| 22 | Hidden-content search (OCR, semantic, MCP) | Kensaku README comparison table | None | Feature absent | L | Heavy deps | MIT | REJECT. Out of scope and heavy. |
| 23 | Plugin architecture | `.vendor/terminator-master/terminatorlib/plugin.py` | None | Feature absent | L | Large surface | GPL-2.0 | REJECT. Not justified. |

## Top 5 by user-visible win divided by effort

1. **Terminal search match-case toggle** (row 1, S, ADOPT)
   Add `case_sensitive: bool` to `search::Search` (`crates/flamewm-terminal/src/search.rs:75`). Replace the unconditional `fold()` at `search.rs:31` and `find_in` at `:38` with a branch: fold only when the flag is off. Add a toggle key in the search overlay path in `crates/flamewm-terminal/src/app/overlay.rs`, plus a visible state label. Tests go in the existing `search.rs` test module (`:115-260`), following the case-insensitive test at `:120`. Keep the shortcut name in `crates/flamewm-api` if it is a shared shortcut.

2. **Explorer ranked name search** (row 2, S-M, ADAPT)
   In `crates/flamewm-explorer/src/search.rs`, after `walk` returns hits (`:170` `search_tree`), stable-sort by a rank key: exact match 0, prefix 1, substring 2, then name length, then path. Keep `name_matches` (`:42`) as the filter and do not add fuzzy matching. Tests go in the module at `search.rs:336-420`, extending `matches_case_insensitively_and_ignores_blank_queries` at `:351`. Cost is a sort of at most `MAX_HITS` (`:17`).

3. **Explorer truncation notice** (row 3, S, ADAPT, UNVERIFIED UI)
   `SearchOutcome.truncated` (`search.rs:36`) already exists. Verify where the explorer consumes `SearchEvent` in `crates/flamewm-explorer/src/app/` and whether the status line shows truncation. If not, add one status string through the existing `activity` field (`app/input.rs:399` pattern). Test by asserting the string for a `truncated: true` outcome.

4. **Terminal keyboard pane resize** (row 4, M, ADAPT)
   Add two shortcuts (grow and shrink focused split) to the shared registry in `crates/flamewm-platform` and `crates/flamewm-api`, per the AGENTS shortcut rule. Handle them in `crates/flamewm-terminal/src/app/keyboard.rs`. In `crates/flamewm-terminal/src/app/layout.rs`, adjust the ratio of the focused split node, clamp with `split_fits` (`:279`) and `split_rect` (`:235`), then call `relayout` (`:299`). Tests go in `crates/flamewm-terminal/src/app/tests.rs` (1085 lines), beside the existing split tests.

5. **Flamefetch redraw on resize** (row 5, M, ADAPT, low win)
   `exec_shell` (`crates/flamewm-terminal/src/fetch.rs:161`) replaces the process before any resize can be seen. Options: keep the process until the first keypress, or re-render before exec using a fresh `terminal_dimensions` (`fetch.rs:407`). Test the layout math only, with `render` (`fetch.rs:515`) into a `Vec<u8>` writer. Do not add a sleep or poll loop.

## Checks

- Commands run: `ctx_batch_execute` reads and `grep` on `.vendor/**` and `crates/flamewm-explorer`, `crates/flamewm-terminal`. Exits 0 except one unmatched glob (`src/*.cpp`, no such path in ark-master).
- Not run: `cargo build`, `cargo test`, any runtime launch (per assignment).
- Not checked: `app/overlay.rs` UI render path, `app/input.rs` remaining arrow keys in grid view (UNVERIFIED), Ark conflict and progress code, Kensaku `SearchBar.tsx`, FileWheel `wheel.rs` keyboard handler in full.

Risk: rows 3 and 14 are UNVERIFIED, and every "gap" above is source-level only.
