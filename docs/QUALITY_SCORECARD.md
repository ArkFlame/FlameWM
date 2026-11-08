# FlameWM Quality Scorecard

Snapshot: 2026-11-08, `main` at `cb5999d1`, measured on the **working tree**, which has uncommitted edits in about 50 files (see [Caveats](#caveats)). Scores are 0-100, higher is better. Architecture context: [ARCHITECTURE_MAP.md](ARCHITECTURE_MAP.md). Build and idle evidence: [BUILD_TIME_2026-11-05.md](BUILD_TIME_2026-11-05.md), [GPU_BACKENDS.md](GPU_BACKENDS.md).

## Repository summary

| Dimension | Weight | LOC-weighted score | Meaning |
|---|---|---|---|
| Modularity (M) | 1/6 | 76.3 | Fan-out and layer direction |
| Size discipline (S) | 1/6 | **35.5** | Long functions, very large crates |
| Error handling (E) | 1/6 | 69.9 | `unwrap`/`expect`/`#[allow]` in production code |
| Test density (T) | 1/6 | 96.2 | `#[test]` per kLOC |
| Duplication (D) | 1/6 | 99.9 | Duplicated function bodies (>= 6 lines) |
| Doc coverage (Dc) | 1/6 | 70.5 | `///` and `//!` lines per production LOC |
| **Overall** | | **74.6** | Mean of the six dimensions, LOC-weighted |

Key counts (production code, test code excluded):

- 57 crates, 220,784 production LOC, 10,289 functions, 243 normal workspace edges.
- **0 dependency cycles.** 4 declared-layer violations (section [Layer violations](ARCHITECTURE_MAP.md#layer-violations)).
- **158 functions longer than 100 lines**, the main source of the low S score. 11 functions nest 8 or more brace levels.
- Duplication is almost absent: the largest shared bodies are `initial_settings` (4 crates, 14 lines) and `admit` (2 crates, 25 lines). This is the strongest dimension.
- Testing is dense (T 96). It is not the risk. The risk is function size.

## Method

Generator: `scratchpad/gen_metrics.py` (not committed; rerun with `python3 -I`). It reads `cargo metadata --no-deps` for the crate graph, then scans each crate's `*.rs`.

- **Test vs production.** Lines inside `#[cfg(test)]` blocks, and whole files under `tests/`, `tests.rs` or `*_tests.rs`, count as test. Blocks are found by brace matching after stripping string literals and `//` comments.
- **Functions.** Regex on `fn` declarations; body from brace matching. Nesting is the maximum brace depth in a body. Depth 8 means 7 levels inside the function body.
- **Duplicates.** Each function body with at least 6 lines is whitespace-stripped and MD5-hashed. A body shared by two or more crates is duplicate. Dup LOC is the crate's lines in such bodies.
- **Counts.** `.unwrap()`, `.expect(`, `.clone()`, `#[allow]`/`#[expect]` in production code. Doc lines are `///` or `//!`. Tests are `#[test]` attributes.

Score formulas (per crate, then clamped to 0-100):

- **M** = 100 - 5 x max(0, fan-out - 6) - 15 x (edges to a higher layer tier)
- **S** = 100 - 4 x (functions > 100 lines) - 0.2 x max(0, longest function - 100) - max(0, prod LOC - 6000) / 300
- **E** = 100 - 5 x unwrap - 1 x expect - 1 x `#[allow]`
- **T** = min(100, 100 x (`#[test]` per kLOC) / 8). Full credit at 8 tests per kLOC, a chosen threshold.
- **D** = 100 - 100 x dup LOC / prod LOC
- **Dc** = min(100, 100 x doc lines x 10 / prod LOC). Full credit at 1 doc line per 10 production LOC, a proxy and not rustdoc coverage.
- **Overall** = (M + S + E + T + D + Dc) / 6

Limits of the method:

- Brace matching ignores char literals containing `{` or `}`. Expect small (single-digit percent) errors in function lengths.
- D only sees bodies of 6 or more lines, so it is a lower bound.
- Dc counts doc lines, not documented items.
- S is strict on purpose. A crate with no long functions gets no size penalty beyond its LOC.

## Worst 15 crates and refactors

Ranked by overall score, lowest first. Size: **S** = under a day, one file or one function. **M** = one module split or interface change. **L** = crate boundary or event-loop restructure, needs its own plan.

| # | Crate | Overall | Weakest | Evidence | Refactor | Size |
|---|---|---|---|---|---|---|
| 1 | `flamewm-shell` | 51 | S 0, M 25 | 21 fan-out; `route_event` 437 lines (`event_router/route.rs:23`); `apply_controls` 307 lines (`app/control.rs:112`); `execute` 350 lines (`main.rs:585`) | Split the event router and control application into one handler per control group behind a dispatch table. Move `admit` (duplicated with desktop, 25 lines) to `flamewm-ui-core`. | L |
| 2 | `flamewm-desktop` | 52 | S 0, M 25 | 21 fan-out; `main` 362 lines (`main.rs:127`); `admit`, `rerun_tree`, `stage_tree` duplicated with shell and settings | Separate bootstrap from the run loop in `main`. Give the shared tree helpers one owner in `flamewm-ui-core`. | M |
| 3 | `flamewm-x11-canary` | 58 | S 0, T 20 | 10,121 production LOC with only 157 test LOC. `run_t17` 421 lines (`scenarios/t15_t18.rs:951`); largest file 1,865 lines | Replace per-scenario functions with a declarative scenario table and one runner. This is test tooling, so the risk is low. | M |
| 4 | `flamewm-settings` | 64 | S 0, E 53 | `parse_action` 452 lines (`actions.rs:373`); `execute` 335 (`app_impl_1.rs:494`); `sync_nodes` 314 (`app_impl_2.rs:659`) | Turn `parse_action` into a `(key, parser)` table. Split `execute` per settings page. | M |
| 5 | `flamewm-platform` | 65 | E 0, S 43 | 88 `expect`, the highest in the workspace; `shortcuts.rs:119` `default` is a 276-line literal table | Convert hot-path `expect`s to typed errors. Keep the literal table and move it to a per-category module. | S |
| 6 | `flamewm-wm-x11` | 67 | S 0, M 40 | `run_session` 478 lines (`runtime.rs:125`); `manage` 381 (`wm/manage.rs:209`); 18 fan-out | Extract event dispatch, timers and state updates from `run_session` into named stages. This is WM authority code, so verify with the X11 canary before merging. | L |
| 7 | `flamewm-render-compiler` | 68 | S 0, Dc 19 | `split_items` 671 lines (`css.rs:85`), the single largest function in the workspace | Replace the match ladder with a tokenizer state machine. Keep the 71 existing tests as the regression gate. | M |
| 8 | `flamewm-wm-wayland` | 72 | S 0, M 65 | `init_backend` 255 lines (`drm/init.rs:20`); 126 `clone` calls; 13 functions over 100 lines | Split DRM init into stages. Replace clones on the frame path with borrows. | M |
| 9 | `flamewm-render-x11` | 74 | S 0, E 51 | `event_loop` 642 lines (`app/events.rs:400`); `paint` 432 (`app/render/paint.rs:4`); `new_surface` 431 (`app/mod.rs:328`); 1,313 `unsafe` tokens | Dispatch events through a handler table. Split `paint` by op family. Put the Xlib FFI calls behind a safe wrapper module, since this crate is the declared unsafe boundary. | L |
| 10 | `flamewm-explorer` | 75 | S 46, M 45 | 17 direct workspace dependencies, the widest fan-out after desktop and shell | Reach platform and render services through existing facades instead of direct crates. Confirm each dependency before changing it. | M |
| 11 | `flamewm-control-wire` | 75 | Dc 7 | 2,853 production LOC with almost no doc lines; `#[test]` count is healthy | Document the public wire types and message invariants. Doc-only, no behaviour change. | S |
| 12 | `flamewm-ui` | 76 | Dc 2 | 442 production LOC across 30 files, almost no docs | Add module docs for the public widget API. | S |
| 13 | `flamewm-integrations-linux` | 79 | S 28 | 121 `clone` calls; `icons/mod.rs` 1,139 lines, the crate's largest file | Split the icon service by cache, lookup and load. Audit clones on the icon-lookup path only. | M |
| 14 | `flamewm-terminal` | 80 | S 58 | 9 fan-out; 4 functions over 100 lines | Split the overlay and selection paths by responsibility. Hold this until its uncommitted edits land, since the files are in the working tree. | M |
| 15 | `flamewm-apps` | 82 | T 0 | 73 production LOC, no tests | Add one smoke test. The low rank is a formula effect on a tiny crate, not a risk. | S |

Recommended order, by value per effort:

1. Fix the 4 layer violations. Each is an S change, and they keep the "pure core" claim true. See [ARCHITECTURE_MAP.md](ARCHITECTURE_MAP.md#layer-violations).
2. Dedupe `admit`, `rerun_tree` and `stage_tree` into `flamewm-ui-core` (S). This touches three crates that have uncommitted work, so do it after they land.
3. Split `route_event`, `run_session` and `event_loop` (L each). Each needs a canary run before and after.

## Caveats

- **Uncommitted work.** About 50 files in shell, desktop, settings, explorer, terminal, platform, wm-x11, wm-wayland, integrations-linux, render-x11, lock and x11-canary are modified in the working tree. The scores describe that tree, so refactors in those crates can conflict with pending edits.
- **Layer model is an assessment.** The 0/1/2 tiers are assigned by this review from crate names and roles (see [ARCHITECTURE_MAP.md](ARCHITECTURE_MAP.md)). No layer config exists in the repo. Violations therefore depend on that model.
- **`flamewm-apps` and `flamewm-ui` look worse than they are** because of small size. The formulas are not normalized for size below about 500 LOC.
- **No runtime evidence.** This is a static scorecard. Performance and behaviour claims are not made here.
- **Not checked.** Per-function cyclomatic complexity, `clippy` lints, and rustdoc coverage were not measured.

## Full crate table

Columns: fan-in and fan-out are workspace edges; `unwrap`, `expect`, `clone` and `allow` count production code only; `fn>100` is functions over 100 lines; `nest>=8` is functions at depth 8 or more; `Dup LOC` is duplicate-body lines; `Doc lines` is `///`/`//!` count. Scores M, S, E, T, D and Dc are as defined in [Method](#method).

| Rank | Crate | Layer | Fan-in | Fan-out | Prod LOC | Test LOC | unwrap | expect | clone | #[allow] | fn>100 | fn nest>=8 | Max fn | Dup LOC | Doc lines | Tests | M | S | E | T | D | Dc | **Overall** |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | flamewm-shell | 2 | 1 | 21 | 21749 | 13273 | 10 | 50 | 201 | 1 | 18 | 2 | 437 | 47 | 1755 | 590 | 25 | 0 | 0 | 100 | 100 | 81 | **51** |
| 2 | flamewm-desktop | 2 | 1 | 21 | 10018 | 2681 | 0 | 47 | 88 | 3 | 11 | 2 | 362 | 47 | 364 | 167 | 25 | 0 | 50 | 100 | 100 | 36 | **52** |
| 3 | flamewm-x11-canary | 2 | 0 | 0 | 10121 | 157 | 0 | 0 | 16 | 3 | 19 | 1 | 421 | 0 | 322 | 16 | 100 | 0 | 97 | 20 | 100 | 32 | **58** |
| 4 | flamewm-settings | 2 | 1 | 10 | 7842 | 5406 | 0 | 47 | 46 | 0 | 6 | 0 | 452 | 22 | 403 | 222 | 80 | 0 | 53 | 100 | 100 | 51 | **64** |
| 5 | flamewm-platform | 1 | 6 | 1 | 7678 | 5417 | 4 | 88 | 67 | 1 | 4 | 0 | 276 | 0 | 355 | 332 | 100 | 43 | 0 | 100 | 100 | 46 | **65** |
| 6 | flamewm-wm-x11 | 1 | 1 | 18 | 20318 | 10722 | 0 | 17 | 32 | 5 | 22 | 0 | 478 | 10 | 1739 | 560 | 40 | 0 | 78 | 100 | 100 | 86 | **67** |
| 7 | flamewm-render-compiler | 0 | 0 | 3 | 2504 | 1311 | 2 | 0 | 8 | 0 | 3 | 1 | 671 | 0 | 48 | 60 | 100 | 0 | 90 | 100 | 100 | 19 | **68** |
| 8 | flamewm-wm-wayland | 1 | 0 | 13 | 15654 | 5749 | 1 | 13 | 126 | 12 | 13 | 0 | 255 | 36 | 1903 | 302 | 65 | 0 | 70 | 100 | 100 | 100 | **72** |
| 9 | flamewm-render-x11 | 1 | 2 | 5 | 24099 | 11215 | 0 | 32 | 19 | 17 | 14 | 2 | 642 | 34 | 2260 | 436 | 100 | 0 | 51 | 100 | 100 | 94 | **74** |
| 10 | flamewm-explorer | 2 | 1 | 17 | 8770 | 4484 | 0 | 25 | 136 | 0 | 5 | 1 | 225 | 14 | 731 | 228 | 45 | 46 | 75 | 100 | 100 | 83 | **75** |
| 11 | flamewm-control-wire | 0 | 6 | 2 | 2853 | 1124 | 0 | 0 | 59 | 0 | 6 | 0 | 255 | 0 | 19 | 50 | 100 | 45 | 100 | 100 | 100 | 7 | **75** |
| 12 | flamewm-ui | 1 | 1 | 1 | 442 | 21 | 0 | 0 | 2 | 0 | 0 | 0 | 10 | 0 | 1 | 2 | 100 | 100 | 100 | 57 | 100 | 2 | **76** |
| 13 | flamewm-integrations-linux | 1 | 7 | 6 | 13901 | 6861 | 0 | 9 | 121 | 3 | 5 | 0 | 228 | 6 | 792 | 298 | 100 | 28 | 88 | 100 | 100 | 57 | **79** |
| 14 | flamewm-terminal | 2 | 1 | 9 | 8163 | 4176 | 0 | 14 | 39 | 1 | 4 | 0 | 192 | 8 | 448 | 191 | 85 | 58 | 85 | 100 | 100 | 55 | **80** |
| 15 | flamewm-apps | 2 | 0 | 8 | 73 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 51 | 0 | 9 | 0 | 90 | 100 | 100 | 0 | 100 | 100 | **82** |
| 16 | flamewm-task-manager | 2 | 1 | 7 | 1694 | 407 | 0 | 4 | 5 | 0 | 2 | 0 | 161 | 14 | 45 | 27 | 95 | 80 | 96 | 100 | 99 | 27 | **83** |
| 17 | flamewm-ui-layout-core | 0 | 1 | 1 | 394 | 62 | 0 | 0 | 4 | 0 | 1 | 0 | 101 | 0 | 3 | 4 | 100 | 96 | 100 | 100 | 100 | 8 | **84** |
| 18 | flamewm-screenshot | 2 | 0 | 2 | 2120 | 702 | 0 | 0 | 2 | 1 | 2 | 0 | 109 | 0 | 62 | 40 | 100 | 90 | 99 | 100 | 100 | 29 | **86** |
| 19 | flamewm-control-core | 0 | 14 | 2 | 769 | 797 | 0 | 3 | 3 | 0 | 1 | 0 | 196 | 0 | 42 | 21 | 85 | 77 | 97 | 100 | 100 | 55 | **86** |
| 20 | flamewm-integrations-core | 0 | 2 | 1 | 765 | 342 | 0 | 0 | 14 | 0 | 0 | 0 | 35 | 0 | 12 | 16 | 100 | 100 | 100 | 100 | 100 | 16 | **86** |
| 21 | flamewm-desktop-core | 0 | 5 | 3 | 2236 | 834 | 0 | 3 | 19 | 1 | 0 | 0 | 95 | 0 | 92 | 69 | 85 | 100 | 96 | 100 | 100 | 41 | **87** |
| 22 | flamewm-lock | 2 | 0 | 10 | 5491 | 2808 | 0 | 11 | 29 | 0 | 2 | 0 | 154 | 36 | 446 | 175 | 80 | 81 | 89 | 100 | 99 | 81 | **88** |
| 23 | flamewm-finder | 2 | 1 | 8 | 3408 | 977 | 0 | 4 | 23 | 0 | 2 | 0 | 157 | 43 | 222 | 75 | 90 | 81 | 96 | 100 | 99 | 65 | **88** |
| 24 | flamewm-render-core | 0 | 8 | 1 | 5499 | 4968 | 0 | 0 | 3 | 3 | 6 | 1 | 158 | 12 | 414 | 166 | 100 | 64 | 97 | 100 | 100 | 75 | **89** |
| 25 | flamewm-settings-core | 0 | 2 | 4 | 1215 | 1255 | 0 | 0 | 10 | 0 | 0 | 0 | 50 | 0 | 61 | 26 | 85 | 100 | 100 | 100 | 100 | 50 | **89** |
| 26 | flamewm-wm | 1 | 1 | 7 | 2314 | 825 | 0 | 0 | 25 | 0 | 1 | 1 | 120 | 6 | 124 | 36 | 95 | 92 | 100 | 100 | 100 | 54 | **90** |
| 27 | flamewm-x11-desktop | 1 | 1 | 2 | 2245 | 943 | 0 | 0 | 5 | 0 | 1 | 0 | 102 | 10 | 108 | 49 | 100 | 96 | 100 | 100 | 100 | 48 | **91** |
| 28 | flamewm-widgets-core | 0 | 1 | 2 | 2025 | 784 | 0 | 1 | 6 | 0 | 1 | 0 | 101 | 0 | 103 | 54 | 100 | 96 | 99 | 100 | 100 | 51 | **91** |
| 29 | flamewm-applications | 1 | 9 | 2 | 1765 | 884 | 0 | 0 | 17 | 0 | 0 | 0 | 73 | 0 | 83 | 47 | 100 | 100 | 100 | 100 | 100 | 47 | **91** |
| 30 | flamewm-conversion-core | 0 | 2 | 1 | 1016 | 319 | 0 | 0 | 0 | 0 | 2 | 0 | 129 | 0 | 59 | 18 | 100 | 86 | 100 | 100 | 100 | 58 | **91** |
| 31 | flamewm-imageviewer | 2 | 0 | 7 | 1007 | 390 | 0 | 4 | 5 | 0 | 1 | 0 | 108 | 14 | 60 | 23 | 95 | 94 | 96 | 100 | 99 | 60 | **91** |
| 32 | flamewm-remote-fs | 0 | 1 | 1 | 1780 | 718 | 0 | 1 | 33 | 0 | 0 | 0 | 77 | 0 | 99 | 33 | 100 | 100 | 99 | 100 | 100 | 56 | **92** |
| 33 | flamewm-control-dbus | 1 | 13 | 5 | 678 | 746 | 0 | 1 | 6 | 0 | 0 | 0 | 79 | 0 | 34 | 35 | 100 | 100 | 99 | 100 | 100 | 50 | **92** |
| 34 | flamewm-tray | 1 | 1 | 2 | 1077 | 540 | 0 | 0 | 20 | 0 | 0 | 0 | 38 | 0 | 65 | 25 | 100 | 100 | 100 | 100 | 100 | 60 | **93** |
| 35 | flamewm-archiver-app | 2 | 0 | 4 | 657 | 261 | 0 | 4 | 6 | 0 | 0 | 0 | 79 | 0 | 41 | 11 | 100 | 100 | 96 | 100 | 100 | 62 | **93** |
| 36 | flamewm-assistant | 2 | 0 | 9 | 2686 | 1338 | 0 | 4 | 26 | 0 | 1 | 0 | 118 | 43 | 245 | 76 | 85 | 92 | 96 | 100 | 98 | 91 | **94** |
| 37 | flamewm-assistant-core | 0 | 1 | 0 | 2304 | 1498 | 0 | 1 | 8 | 0 | 1 | 0 | 144 | 0 | 187 | 63 | 100 | 87 | 99 | 100 | 100 | 81 | **94** |
| 38 | flamewm-ui-x11 | 1 | 11 | 7 | 1876 | 577 | 0 | 2 | 10 | 3 | 0 | 0 | 62 | 22 | 140 | 22 | 95 | 100 | 95 | 100 | 99 | 75 | **94** |
| 39 | flamewm-debug | 0 | 4 | 0 | 206 | 40 | 0 | 0 | 1 | 0 | 0 | 0 | 29 | 0 | 34 | 1 | 100 | 100 | 100 | 61 | 100 | 100 | **94** |
| 40 | flamewm-downloads-core | 0 | 1 | 0 | 305 | 109 | 0 | 0 | 0 | 0 | 0 | 0 | 39 | 0 | 22 | 8 | 100 | 100 | 100 | 100 | 100 | 72 | **95** |
| 41 | flamewm-ui-core | 0 | 18 | 3 | 2281 | 1796 | 0 | 0 | 1 | 0 | 1 | 0 | 118 | 0 | 190 | 107 | 100 | 92 | 100 | 100 | 100 | 83 | **96** |
| 42 | flamewm-archiver | 1 | 1 | 0 | 1514 | 1333 | 0 | 0 | 3 | 1 | 0 | 0 | 76 | 0 | 112 | 45 | 100 | 100 | 99 | 100 | 100 | 74 | **96** |
| 43 | flamewm-session-core | 0 | 6 | 1 | 1408 | 1131 | 0 | 0 | 8 | 1 | 1 | 0 | 105 | 0 | 119 | 50 | 100 | 95 | 99 | 100 | 100 | 85 | **96** |
| 44 | flamewm-shell-core | 0 | 2 | 3 | 2549 | 1871 | 0 | 1 | 22 | 0 | 0 | 0 | 83 | 9 | 267 | 126 | 85 | 100 | 99 | 100 | 100 | 100 | **97** |
| 45 | flamewm-image-core | 0 | 13 | 1 | 2286 | 2044 | 0 | 0 | 0 | 0 | 1 | 0 | 118 | 0 | 213 | 86 | 100 | 92 | 100 | 100 | 100 | 93 | **98** |
| 46 | flamewm-skin | 0 | 6 | 0 | 1439 | 613 | 0 | 0 | 0 | 0 | 0 | 0 | 45 | 8 | 126 | 41 | 100 | 100 | 100 | 100 | 99 | 88 | **98** |
| 47 | flamewm-profiler | 0 | 7 | 0 | 1039 | 605 | 0 | 0 | 1 | 0 | 1 | 0 | 114 | 0 | 102 | 25 | 100 | 93 | 100 | 100 | 100 | 98 | **98** |
| 48 | flamewm-clipboard | 0 | 5 | 0 | 825 | 299 | 0 | 1 | 4 | 0 | 0 | 0 | 78 | 0 | 76 | 24 | 100 | 100 | 99 | 100 | 100 | 92 | **98** |
| 49 | flamewm-weather | 0 | 2 | 1 | 694 | 403 | 0 | 0 | 1 | 0 | 0 | 0 | 44 | 0 | 59 | 15 | 100 | 100 | 100 | 100 | 100 | 85 | **98** |
| 50 | flamewm-reactor | 0 | 9 | 0 | 655 | 278 | 0 | 11 | 2 | 2 | 0 | 0 | 42 | 9 | 84 | 26 | 100 | 100 | 87 | 100 | 99 | 100 | **98** |
| 51 | flamewm-ribbons | 0 | 1 | 0 | 1230 | 1036 | 1 | 0 | 0 | 2 | 0 | 0 | 88 | 0 | 128 | 46 | 100 | 100 | 93 | 100 | 100 | 100 | **99** |
| 52 | flamewm-api | 0 | 42 | 0 | 2733 | 820 | 0 | 0 | 0 | 0 | 0 | 0 | 57 | 14 | 295 | 61 | 100 | 100 | 100 | 100 | 99 | 100 | **100** |
| 53 | flamewm-ui-document | 1 | 6 | 5 | 878 | 789 | 0 | 0 | 0 | 0 | 0 | 0 | 41 | 0 | 108 | 30 | 100 | 100 | 100 | 100 | 100 | 100 | **100** |
| 54 | flamewm-xsettings | 0 | 1 | 1 | 705 | 764 | 0 | 0 | 11 | 0 | 0 | 0 | 46 | 0 | 102 | 32 | 100 | 100 | 100 | 100 | 100 | 100 | **100** |
| 55 | flamewm-window-core | 0 | 2 | 1 | 506 | 585 | 0 | 0 | 0 | 0 | 0 | 0 | 49 | 0 | 70 | 39 | 100 | 100 | 100 | 100 | 100 | 100 | **100** |
| 56 | flamewm-dbus-reactor | 0 | 14 | 2 | 170 | 21 | 0 | 0 | 0 | 0 | 0 | 0 | 30 | 0 | 18 | 2 | 100 | 100 | 100 | 100 | 100 | 100 | **100** |
| 57 | flamewm-audio-helper | 1 | 0 | 2 | 155 | 236 | 0 | 0 | 0 | 0 | 0 | 0 | 41 | 0 | 15 | 10 | 100 | 100 | 100 | 100 | 100 | 97 | **100** |

## Largest files, longest functions, duplicate bodies

### Largest production files

| Rank | File | Prod LOC |
|---|---|---|
| 1 | crates/flamewm-x11-canary/src/scenarios/interaction_regressions.rs | 1865 |
| 2 | crates/flamewm-desktop/src/widgets/mod.rs | 1351 |
| 3 | crates/flamewm-x11-canary/src/scenarios/t15_t18.rs | 1347 |
| 4 | crates/flamewm-render-x11/src/surface_controller.rs | 1329 |
| 5 | crates/flamewm-render-x11/src/app/events.rs | 1207 |
| 6 | crates/flamewm-lock/src/run.rs | 1202 |
| 7 | crates/flamewm-render-x11/src/app/mod.rs | 1173 |
| 8 | crates/flamewm-assistant/src/lib.rs | 1146 |
| 9 | crates/flamewm-platform/src/settings.rs | 1143 |
| 10 | crates/flamewm-integrations-linux/src/icons/mod.rs | 1139 |
| 11 | crates/flamewm-wm-wayland/src/state.rs | 1132 |
| 12 | crates/flamewm-wm-x11/src/lib.rs | 1096 |
| 13 | crates/flamewm-desktop/src/projection.rs | 1076 |
| 14 | crates/flamewm-x11-canary/src/scenarios/explicit_shell.rs | 1073 |
| 15 | crates/flamewm-render-x11/src/overlay.rs | 1069 |

### Functions over 100 lines (top 20)

| Rank | Lines | Max depth | Location | Fn | Crate |
|---|---|---|---|---|---|
| 1 | 671 | 8 | crates/flamewm-render-compiler/src/css.rs:85 | split_items | flamewm-render-compiler |
| 2 | 642 | 8 | crates/flamewm-render-x11/src/app/events.rs:400 | event_loop | flamewm-render-x11 |
| 3 | 478 | 6 | crates/flamewm-wm-x11/src/runtime.rs:125 | run_session | flamewm-wm-x11 |
| 4 | 452 | 4 | crates/flamewm-settings/src/actions.rs:373 | parse_action | flamewm-settings |
| 5 | 437 | 6 | crates/flamewm-shell/src/event_router/route.rs:23 | route_event | flamewm-shell |
| 6 | 432 | 8 | crates/flamewm-render-x11/src/app/render/paint.rs:4 | paint | flamewm-render-x11 |
| 7 | 431 | 6 | crates/flamewm-render-x11/src/app/mod.rs:328 | new_surface | flamewm-render-x11 |
| 8 | 421 | 4 | crates/flamewm-x11-canary/src/scenarios/t15_t18.rs:951 | run_t17 | flamewm-x11-canary |
| 9 | 381 | 4 | crates/flamewm-wm-x11/src/wm/manage.rs:209 | manage | flamewm-wm-x11 |
| 10 | 362 | 6 | crates/flamewm-desktop/src/main.rs:127 | main | flamewm-desktop |
| 11 | 351 | 6 | crates/flamewm-x11-canary/src/commands/interaction_perf.rs:29 | cmd_interaction_perf | flamewm-x11-canary |
| 12 | 350 | 5 | crates/flamewm-shell/src/main.rs:585 | execute | flamewm-shell |
| 13 | 335 | 5 | crates/flamewm-settings/src/app_impl_1.rs:494 | execute | flamewm-settings |
| 14 | 314 | 4 | crates/flamewm-settings/src/app_impl_2.rs:659 | sync_nodes | flamewm-settings |
| 15 | 307 | 8 | crates/flamewm-shell/src/app/control.rs:112 | apply_controls | flamewm-shell |
| 16 | 299 | 6 | crates/flamewm-x11-canary/src/commands/j13.rs:13 | cmd_j13 | flamewm-x11-canary |
| 17 | 276 | 2 | crates/flamewm-platform/src/shortcuts.rs:119 | default | flamewm-platform |
| 18 | 272 | 10 | crates/flamewm-x11-canary/src/commands/sr.rs:14 | cmd_sr05 | flamewm-x11-canary |
| 19 | 267 | 7 | crates/flamewm-shell/src/quick_controls/host/run.rs:11 | run_quick_control_host | flamewm-shell |
| 20 | 255 | 6 | crates/flamewm-wm-wayland/src/drm/init.rs:20 | init_backend | flamewm-wm-wayland |


### Duplicate bodies shared by crates (top 10)

| Rank | Lines | Crates | Name (sample) | Crates sharing body |
|---|---|---|---|---|
| 1 | 14 | 4 | initial_settings | assistant, finder, imageviewer, task-manager |
| 2 | 25 | 2 | admit | desktop, shell |
| 3 | 12 | 3 | rerun_tree | desktop, settings, shell |
| 4 | 10 | 3 | stage_tree | desktop, settings, shell |
| 5 | 13 | 2 | drain | assistant, finder |
| 6 | 12 | 2 | hole_row_span | render-core, render-x11 |
| 7 | 12 | 2 | default | render-x11, ui-x11 |
| 8 | 10 | 2 | send_state | wm-x11, x11-desktop |
| 9 | 10 | 2 | nonblocking_pair | assistant, finder |
| 10 | 10 | 2 | from | render-x11, ui-x11 |
