# FlameWM 0.1.1 — CommandCode / DeepSeek V4.1 CPU Crisis: Autonomous Optimization Handoff

**Prepared:** 2026-10-23  |  **Task:** Implement, measure, and verify an aggressive CPU reduction across FlameWM, terminal, window management, desktop interaction, Start menu, and X11 rendering.  **Primary target:** **≥95% reduction** in avoidable CPU for each reproducible workload, subject to visual correctness and responsiveness. **Do not invent the result or treat the target as already attained.**

**Source basis:** User-supplied `flamewm_1312_23_10_26.tar.gz` and `command-code-session-963a7b2b.md`, supplemented by read-only examination of extracted archive files and official Rust/Linux profiling documentation. This document is **source-grounded planning**, not proof of runtime improvements. The *current live Git checkout*, possibly modified after the archive, is authoritative.

**Start the next CommandCode session with this single `.md` as the complete handoff. Continue autonomously until each accepted, measurable goal is reached or a precise evidence-backed environmental blocker has been established. No progress-only exit and no pretending that tests or benchmarks passed.**

## 0. Non-negotiable goal and operating contract

1. **Fix CPU consumption at its producer, not by hiding frames.** The system must stay fully interactive, low latency, consistent with existing FlameWM visuals, fluid under dragging/resizing and terminal typing, and quiet when idle. No blanket 15/30-FPS cap, gratuitous timers, `sleep` in interaction handlers, animation deletion, disabled shadows/transparency or fewer features as a shortcut.
2. **95% is the optimization ambition, not a fact or universal promise.** Calculate each workload as `reduction = 100 * (before_cpu_seconds - after_cpu_seconds) / before_cpu_seconds` under the *same* controlled workload, machine, binary profile, resolution, settings, and capture method. Also report CPU percent of one core, Xephyr/Xorg separately, input latency, paint/damage, and visible correctness. A workload already at ~0% idle cannot meaningfully improve 95%; enforce a low absolute idle budget instead. Distinguish controllable FlameWM CPU from unavoidable Xorg/Xephyr work and non-Flame clients (`htop`, browsers).
3. **Preserve existing behavior and product constitution:** independent Rust X11 WM/desktop; Flame Fusion Explorer; high-DPI task icons; panel, popup, hover, selection rectangle, taskbar, Start, terminal tab/split, window drag/resize, maximize/minimize/restore, simple chamfered bottom-right cast shadows, opacity/blur from live theme settings, clipboard, multi-monitor, true icons/desktop entries, KWallet cross-desktop data safety. Never overwrite user profiles/wallets, alter KDE user defaults, touch personal disks, install a compositor, or silently replace architecture.
4. **Single source of truth per concern.** Semantic feature/controller owns state; one retained document/layout/damage authority owns UI invalidation; renderer owns painting and X uploads; platform owns X11 requests, native surfaces and lifecycle. Optimize the canonical shared primitive first when multiple features are affected. No duplicate per-feature rendering infrastructure and no premature framework rewrite.
5. **Use short, high-value iterations.** Snapshot/reproduce → attribute → add deterministic failing test/probe → smallest first-owner fix → focused tests → measurable sample → visual test → retain/regress. Process all disjoint READY jobs concurrently, not sequentially out of convenience. A builder may report `PATCH_APPLIED_NEEDS_VERIFY`; only a fresh verifier can report `PASS`.
6. **No questions for routine decisions.** Continue through the task frontier and work on other READY jobs when one is blocked. Never evade a permission/safety denial. Never claim hardware-specific tests passed when Xephyr cannot emulate them. Stop only when accepted or blocked with exact evidence.
7. **Final output:** changes in the actual checkout, tests, before/after metrics, visual screenshots/evidence, an optimized working `.deb` if packaging is green, commit(s) if allowed by repository policy, and a concise residual checklist. Do not return a handoff instead of implementing; **this file is the implementation prompt**.

## 1. Fresh source, state and worktree freeze (mandatory before editing)

Repository path reported by previous agents:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
pwd -P
cat AGENTS.md
sed -n '1,200p' .opencode/HARNESS.md
sed -n '1,200p' .opencode/ROUTES.md
sed -n '1,200p' .agents/CONTRACTS.md
sed -n '1,200p' .agents/JOBS.md
sed -n '1,160p' docs/PERFORMANCE.md
sed -n '1,160p' docs/VERIFICATION.md
git status --short --branch
git log -5 --oneline
sha256sum Cargo.lock
date -Is
```

The previous CommandCode conversation's *last completed commit* was `b9311046` (52 files) before further wallet work. **Do not assume it remains HEAD**: the supplied later archive and subsequent wallet task may reflect a newer worktree. User owns all dirty/untracked content. Do not `git reset --hard`, `git clean`, blindly untar overlays, change major toolchains, change `Cargo.lock` without justification, or overwrite a parallel agent's edits. Compare the current checkout with archive/report assertions before using locations/line numbers.

Archive characteristics confirmed in the supplied snapshot: workspace version `0.1.1`, Rust edition 2024, Rust minimum 1.85, 41 crates, `Cargo.toml` with release `opt-level=3`, Thin LTO, one codegen unit, `panic=abort`, and a `[profile.fast]` suitable for quicker experiments but **not directly comparable** with full release. Use `--locked` for release/acceptance. The source archive is not necessarily a complete build/runtime-assets checkout; the live repository and its build scripts are canonical.

## 2. Measured earlier evidence: why this task is urgent

These are **historical observations from the supplied CommandCode transcript, not new measurements on the October 23 archive**. They were taken on a nested Xephyr display around 1350×641 with mixed XTEST/UI scripted workloads; preserve that distinction. CPU percentages are approximate **% of one logical core** (100 = one fully utilized core).

| Workload / probe | Total measured CPU | FlameWM responsibility | X server | Interpretation |
|---|---:|---:|---:|---|
| `idle_desktop_only`, sequence-1 | 0.00% | ~0 | ~0 | Existing event-driven idle may be excellent under some controlled conditions. Do not claim permanent high idle CPU based only on uncontrolled runs. |
| `selection_desktop`, phases-2 | 63.49% | desktop 38.54% | Xephyr 24.95% | **High-confidence P0 hotspot**. |
| `selection_desktop`, phases-3 | 47.27% | desktop 27.74% | Xephyr 19.53% | Repeated selection paints remain expensive. |
| `selection_drag`, sequence-1 | 63.81% | desktop 37.64% | Xephyr 25.98% | XDamage ~1,113/s desktop + ~1,115/s root; likely duplicate observability, **not 2,230 distinct paints**. |
| `start_toggle`, phases-3 | 32.26% | desktop 6.96%, shell 2.96%, WM 0.59% | Xephyr 21.75% | Unexpected **desktop** work while shell Start toggles. |
| `start_open_settled`, phases-3 | 31.59% | desktop 9.2%, shell 0.2% | Xephyr 22.19% | **Potential repaint/reaction bug** while Start is merely open; other `menu_open` probe was 0%, so verify the exact initial state. |
| `terminal_selection`, phases-3 | 54.31% | terminal 14.53% | Xephyr 23.6%; `htop` 15.74% | Do not blame terminal for `htop` own redraw CPU. |
| `window_move`, phases-3 | 35.9% | WM 4.19%, desktop 1.59%, terminal 3.37%, shell 0.57% | Xephyr 15.63%, `htop` 10.55% | Movement seems to trigger X server, terminal and wallpaper/backdrop consumers more than WM alone. |
| `window_resize`, phases-3 | 47.54% | WM 3.78%, desktop 2.41%, terminal 4.18%, shell 0.97% | Xephyr 22.51%, `htop` 13.63% | Suspect geometry-triggered full rebuild and X uploads. |
| `idle_prompt`, phases-3 | 0.99% | WM 0.12%, shell 0.25%, terminal 0.12% | Xephyr 0.50% | Baseline low; ~29 observed damage events/s may simply be clock/panel or cursor blink. |
| `idle_htop`, phases-3 | 3.4% | terminal 2.5% | Xephyr 0.7%, htop not sampled as contributing in this run | htop usage varies; isolate reader/render scheduling. |
| `held_move`/`held_resize` | inconsistent across runs | sometimes low; sometimes `htop`/terminal dominant | variable | Must compare same visible app state; holding pointer still must not create motion frames. |

The transcript also had `phases-2` measurements with damage probes registering zero because instrumentation setup was inconclusive. Later `phases-3` and `sequence-1` provided live damage counts. Do not treat **raw damage-event counts** as frame counts: distinguish raw event notifications, XDamage rectangles, painting passes, buffer upload area, and geometry changes. Earlier profiler work suffered from `htop` contaminating every phase, startup noise, tests running alongside builds, and mixed screenshot conditions. Strict controls are necessary.

Prior author specifically reported that a `207 ms` WM drag figure was startup profiler noise; a controlled drag's WM CPU time was closer to `~4 ms` per drag. Do **not** regress the final minimal cast shadow/window drag state in pursuit of fictitious WM-local CPU savings.

## 3. P0 technical root-cause candidates — investigate in this order

### P0A — Canonical UI document mutations produce full-surface damage

**Source verified in archive:** `crates/flamewm-render-core/src/model/document.rs`, `RuntimeDocument::touch()` increments revision and merges `SurfaceDamage::Full`. `touch_light()` increments revision without immediately changing the damage latch. Trace **every setter** (`text`, `background`, `border`, `visible`, `position`, `size`, styles, images, scroll) → `touch`/`touch_light` → `SurfaceRuntime::mark_dirty` / `mark_full_dirty` in `crates/flamewm-ui-x11/src/surface_runtime.rs` → layout/paint commands → `flamewm-render-x11` upload. Identify which update needs relayout (text/size/visibility), which only needs repaint (color/selection), and which is a no-op (same value). **The existence of `touch()` is not proof every update invokes it**: classify concrete callers and actual number of document changes per input event.

**Implement if proved:** typed invalidation with coalesced per-surface damage bounds, no-op equality before invalidation, no duplicate layout for paint-only edits, old+new geometry union for moving content, invalidation for text shadow/blur/inset and popup surfaces. Retain one event-loop present owner and never lose damage or clip glyph/selection state. Start with one demonstrably bad consumer (desktop rubber selection) and migrate its entire path before changing other surfaces. Avoid huge renderer API redesign unless a negative regression gate demands it.

**Probes:** document `touch_full`, `touch_light`, `layout_runs`, `paint_passes`, X PutImage upload bytes and area, with feature-flagged/log-disabled counters. Cross-check X server damage; at idle zero invalidation unless timer-driven visual elements genuinely change.

### P0B — Desktop rubber-band selection recomputes geometry and paints *all* rows on motion

**Archive verified:** `crates/flamewm-desktop/src/main.rs`: `projected_cells()` around line 1429 collects all items and constructs a `Vec<Option<...>>` and `BTreeMap<OutputId,BTreeSet<Cell>>`; `update_selection_from_rubber()` around line 3448 **calls `projected_cells` on every selection update**, builds rectangles for all items, then `clear()`+reselects the set; `sync_selection_visuals()` around line 3470 loops over **every** item and calls background/border setters or clears, even unchanged items. `set_selection_overlay` calls separate `visible`, `position`, and `size` operations. Earlier optimization removed some BTreeMap<String,Rect> allocation but did not remove repeated projection or full selection visuals.

**Root owner/solution:** freeze a *gesture-local* projected item rect snapshot at selection start (or durable projection revision cache already owned by desktop layout), and update it only on actual desktop layout/grid/item/output changes. Selection must be incremental by **set symmetric difference**, not clear+repopulate for every pixel. Update only items entering/leaving the selection; cache last selection rect; no-op when pointer coordinate unchanged or selection mask same. Selection rectangle repaints the union of old/new bounds, not entire wallpaper/screen. End gesture commits authoritative selection and drops temporary geometry. Must support cross-monitor, changed grid, rename/drag, item add/remove, Shift/Ctrl semantics and resize/scale mid-gesture. Preserve desktop icon and label layout.

**Test first:** synthetic pointer motion (e.g. 100–1000 identical/no-op events; 100 moving steps across N icons). Count projected layouts, selection-delta mutations, painted pixels and document damage. Test drag across icons, Shift/Ctrl and output geometry mutation. Direct before/after benchmark CPU+damage; aim enormous reduction while selection remains fluid and pixel-correct.

### P0C — Terminal grid `frame()` + `grid_runs()` rebuilt per input and row update budget

**Archive verified:** `crates/flamewm-terminal/src/app.rs::frame()` around line 1206 builds the **whole vector of terminal rows**, pane tree rects and per-pane `grid_runs`; `crates/flamewm-terminal/src/view.rs::grid_runs()` around line 101 scans **all visible cells** and builds new run vectors with string processing; `crates/flamewm-terminal/src/ui.rs` compares each `frame` row to retained previous rows, but for changed rows loops across `POOL_RUNS` and calls multiple document setters (`text`, `position`, `size`, `foreground`, `background`, `font_weight`, `visible`). `Run` strings are cloned to retained UI rows. Even a cursor blink/one keystroke can pay the screen scan and several document mutations. Confirm `frame()` invocation cadence and the PTY reader scheduling before inferring CPU proportions.

**Fix:** at session/screen owner track generation or dirty rows for printed text, scroll, selection, cursor, tab/pane state, resize; *only* convert invalidated terminal lines to render runs. Cache pane composition and layout while pane geometry unchanged. Coalesce PTY data batches (while preserving interactive prompt latency and canonical escape parser ordering); coalesce change events into one visible commit per event-loop dispatch. On selection drag, invalidate old/new selection span rows and affected cells; don't recompose unrelated panes. Preserve Unicode width, combining marks, ANSI escapes, SGR, alt screen, scrollback, selection clipboard, mouse reporting, tabs and vertical/horizontal splits. Do not poll or discard PTY output; do not merely throttle input or introduce noticeable typing latency. If the screen model lacks a correct revision protocol, first add a minimal dirty-row observer at its canonical mutation points and write tests proving no missed updates.

**Tests/probes:** one-character input only updates one line plus cursor; 10KB burst repaint bounded; 200ms idle no redraw except necessary cursor blink; `htop`, `vim`, scrolling, 3-pane split, paste, resize, emoji/wide glyphs, and hidden tabs; verify time-to-echo and frame event count. Test full-screen apps and PTY lifecycle to prevent stale display or tab closure regression.

### P0D — Start-menu toggle/settled state unexpectedly triggers desktop/root repaint

**Evidence:** `start_toggle` and `start_open_settled` runs show Xephyr and `flamewm-desktop` CPU even when most work should be in `flamewm-shell`. In `crates/flamewm-shell/src/runtime.rs`, `open_start_group_measured` / `open_start_group` ~1719, transient popup placement ~1700 and motion close ~1693, and `crates/flamewm-shell/src/app.rs::refresh_dynamic` ~1563, `event_router.rs` ~486 control the relevant path. `crates/flamewm-reactor` is an event-driven loop; periodic host repaints must be explained rather than assumed.

**Trace exact ownership:** Start button → control signal and snapshot → popup projection → native map/unmap/slide → WM Configure/Expose/Property → desktop wallpaper/backdrop redraw → shell panel updates. Count X events and XDamage by *window ID* and type. Check if popup transparency backdrop is forcing root-sized recapture, if hidden surfaces remain rendering, if clock/weather timers accidentally fire per frame, and whether `start_open_settled` measurement included lingering drag/animation or a busy terminal.

**Fix at canonical owner:** coalesce control signals and projection state, stop animation timers when animation completes, render popup only while visible/dirty, invalidate only old/new popup bounds on open/close, cache only correct-size static background, keep high-DPI anchors and menus correct. **Do not disable Start slide, blur, opacity or clock**. Regression: 100 toggles across outputs; never reopens itself; no stale grab; submenu launch works; open but motionless reaches quiescence; no root-size persistent damage.

### P0E — Move/resize/drag and Xorg/Xephyr work

**Source provenance:** `crates/flamewm-wm-x11/src/wm/events.rs` handles `MotionNotify`; `wm/drag.rs`, `wm/resize.rs`, `frame/drag_backdrop.rs`, `frame/titlebar_backdrop.rs`, `wm/shadow.rs`/`wm/paint.rs` may be relevant but filenames and runtime paths must be reverified. Prior development replaced buggy shadow strips with **one bottom-right 45° cast shadow per frame**; previous handoff records configure on motion, wallpaper/root snapshot and image on settle. Preserve this accepted result.

**Trace:** XTest pointer events → grab/drag state → frame geometry/change → shadow window move/stack → EWMH updates → desktop/panel XDamage and clients' ConfigureNotify/Expose → terminal resize or redraw. Count `ConfigureWindow`, `GetImage`, `PutImage`, `GetGeometry`, `TranslateCoordinates`, `QueryTree`, X sync round-trips, and event-loop redispatch. If the WM is already ~4% core during motion but server ~15–35%, moving all work out of the WM will not solve total CPU: find excessive X server resource operations/large uploads in client and backdrop paths.

**Fix only proven excess:** cache known frame/root geometry within gesture; avoid synchronous X round-trips when state is already authoritative; use configure only when integer position/size changes; move native shadow window with frame without recapturing it per motion; defer expensive backdrop recapture to end of interaction; avoid repainting lower/root windows unnecessarily. For resize, maintain correct content geometry, dimensions and snapped layout. Where gesture coalescing is needed, preserve newest position and final release event while coalescing redundant intermediate MotionNotify; never drop ButtonRelease, prevent starvation. No added pointer lag or window disappearing/unmapping. Preserve focus, monitor crossing, maximization, shadow stacking and drag cancellation.

**Regression:** scripted move across wallpaper/another window/popups, fast sweep, held grab, mouse release outside frame, resize in/out, snap unsnap, minimize-maximize-restore, multi-output/mixed-scale where possible. Compare frame positions and screenshots, no missing frames/black titlebar/shadow leaks. `tools/flamewm_move_flicker_drive.py` was present in prior committed source per log; use only after checking current tree.

## 4. P1 high-leverage possibilities, only if P0 evidence points here

- **Native render and X11 upload:** `crates/flamewm-render-x11/src/app/render.rs`, `native/image.rs`, `app/window_backdrop.rs`, `xrender.rs`, `xft.rs`, `overlay.rs`; investigate recreated X pixmaps/pictures every present, Xft text masks/shadow scratch and redundant full-screen RGBA conversion. Earlier Claude optimized scratch reuse, batching and device-pixel alignment; do not undo those improvements. Cache sizes must be bounded by bytes, not unbounded window/item count, and cleaned on close/scale/theme change.
- **Damage / composition:** nearest-neighbor vs good icon raster scaling matters; do not trade quality for speed. Separate root damage triggered by an overlay from actual full desktop redraw. The nested X server may consume CPU because of depth/visual/parent compositor interactions; compare true host Xorg under FlameWM and clean Xephyr without host compositor if possible.
- **App shell:** `async_projection`, icon cache, `projection/*`, `event_router`, quick-controls host, taskbar; verify no hidden panels or quick-controls continue projecting, icon async results are coalesced and unrelated state snapshots are not recopied. Prior code contains legitimate clock/weather/event-driven timers; don't label them leaks without timing.
- **Filesystem/Explorer:** thumbnails, icon decoding, Places/UDisks, transfer worker and mounted volumes must not rescan on pointer motion; use existing typed async results and inotify. Focus only if Explorer appears in CPU top list.
- **IPC / D-Bus:** ensure expensive WM/shell snapshots are fetched only when source revision changed and X11 requests are batched. Existing work already fixed some O(N²) client/window snapshot paths; preserve it.
- **Resource retention:** clipboard, wallpaper buffer sharing, retained images and minimized captures are bounded in previous audits; do not eliminate functional caches without evidence. Memory optimization may make CPU worse if it introduces re-decoding.
- **Release config / instruction count:** compare `[profile.fast]` (not release-equivalent) and release under separate runs. Profile `opt-level`, PGO or alternative allocators *last*, after CPU flamegraph and before/after evidence; do not replace safe architecture with unsafe or make performance depend on exotic runtime flags.

## 5. Measurement protocol — reproducible, not a one-off screenshot

### A. Establish a clean performance baseline before edits

Run a dedicated verifier/profiler agent (read-only on code) on the **current HEAD**. Avoid running Cargo, codegen, kernel indexing, browsers downloading in the background, and competing agent builds during measurement. Preserve an isolated session bus and display; don't inadvertently test the host WM. Keep user data outside scratch fixtures. Fix the sample workload so baseline/after operate on identical binaries except code changes, same X settings, same screen, same panel/clock setting, same clients and client outputs. Use release optimized builds for *both*; instrumentation toggles and loggers off except where explicitly measuring overhead. Warm for 5 seconds and repeat each phase **3–5 times**; save median and spread, 95th percentile responsiveness, system/X server vs apps.

Canonical script:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
export CARGO_BUILD_JOBS=2
cargo build --locked --release --workspace
# On an X desktop, start a visible nested FlameWM session in an isolated display;
# choose an unused :N rather than claiming :97 is free.
FLAMEWM_XEPHYR_BUILD=0 \
FLAMEWM_XEPHYR_PRIVATE_DBUS=1 \
FLAMEWM_XEPHYR_SCREEN=1350x641 \
FLAMEWM_XEPHYR_TEST_APPS=1 \
./scripts/xephyr
```

The script in the snapshot supports `FLAMEWM_XEPHYR_DISPLAY=:N`, `FLAMEWM_XEPHYR_CANARY=1`, `FLAMEWM_XEPHYR_HARD_T06=1`, `FLAMEWM_XEPHYR_SESSION_MODE=1`, `FLAMEWM_XEPHYR_OUTPUT_DIR=...`, `FLAMEWM_XEPHYR_BUILD=0`. **Read `scripts/xephyr --help` and inspect host display occupancy first**. Do not run another nested WM under the user's primary `DISPLAY` and do not kill X processes not created by your own test.

If previous repo-scoped benchmark helpers under `.agents/runtime/handoff-cc-20261022/perf` exist, verify their code/current assumptions and use them; their complete source also appears in `command-code-session-963a7b2b.md`. If absent, create **small permanent reusable** `tools/bench-flamewm-cpu.py` and `tools/verify-flamewm-interactions.py` rather than carrying anonymous temp scripts. Python `Xlib`/XTEST can automate; earlier agent found `python3 -I` hides user-site `python-xlib`, so use a verified Python environment. `xdotool` may be absent.

Workload matrix to measure with isolated cases and X server PID included:

| Case | Deterministic action | Must record |
|---|---|---|
| I1 | desktop alone idle for 30–60s | FlameWM WM/Desktop/Shell CPU, X CPU, wakeups, repaint/damage zero where appropriate |
| I2 | terminal open with no PTY output for 30s | terminal CPU, cursor blink behavior, X damage, no text scan at unrelated idle |
| I3 | controlled 200–2000 keystrokes via XTEST and PTY echo | typing latency, parse time, frame generation count, CPU, visible exact text |
| I4 | `htop` continuously refreshing inside terminal | separate `htop` CPU from terminal and X server; fixed refresh rate |
| S1 | rubber-band desktop selection with 100 consistent motion steps + release | desktop CPU, per-motion layout/selection mutations, X area/damage, correct selected IDs |
| S2 | selection hold without moving for 15s | no repeated repaint / low absolute CPU |
| M1 | toggle Start open/close 30 times after idle | shell + desktop + X CPU, consistent opening/closing, grabs/focus |
| M2 | open Start and leave stationary for 15s | *near-zero* relevant CPU/damage once animation settles |
| W1 | move one idle test window across screen at fixed positions | WM + desktop + shell + terminal + X CPU, ConfigureWindow count, shadow/window correctness |
| W2 | resize test window at fixed geometry steps | same + content/panel geometry, resize latency |
| W3 | hold left mouse on titlebar/resize grip without motion for 15s | low absolute CPU; no fake motion |
| W4 | repeat with 8 windows, shadows/transparency active | same + leaks/visual corruption, bounds per added window |
| R1 | optional native non-Xephyr FlameWM login | only when safe and user-owned; compare host `Xorg`/compositor with nested result |

Use user+sys `utime/stime` deltas from `/proc/PID/stat` with HZ and monotonic elapsed; normalize to one core; identify processes via their `DISPLAY`, executable and role, and include the Xephyr server command. A typical calculation: `cpu_pct_one_core = 100 * (delta_utime + delta_stime) / (CLK_TCK * elapsed_s)`. Avoid double-counting XDamage subscriptions or client PIDs. Read `smaps_rollup`/PSS independently for memory guardrail. For system-wide Xorg hot paths use `perf record -g -p "$PID" -- sleep 10` / `perf report` if permitted; `perf stat` may be restricted. Root/host permission denial is a **blocker for profiling only**, not permission to pretend attribution; proceed with instrumented, nonprivileged FlameWM counters.

**CPU target logic:** every case gets an explicit baseline median. Reach 95% reductions when there are measurable duplicate paints/allocations/syncs. If baseline I1 is 0.00%, goal is bounded idle and zero unnecessary paint, not meaningless −95%. Do not optimize away unavoidable `htop`/Xserver costs by deleting client output or visual features. Verify keyboard/mouse latency **does not worsen**. Keep screenshots at fixed theme and geometry; compare layout, not brittle exact pixels for intentionally animated elements.

### B. Profiling / event instrumentation

When `/proc` samples identify a hotspot, sample that process with flamegraph/perf and add **debug-only** counters at only the few canonical owners needed. Useful fields: `input_events`, `events_coalesced`, `selection_geometry_builds`, `selection_delta_items`, `full_damage_marks`, `rect_damage_marks`, `layout_passes`, `paint_passes`, `x_putimage_bytes`, `x_pixmap_creations`, `x_getimage_calls`, `x_sync_round_trips`, `timer_wakeups`, `scene_reconcile_nodes`, `pty_bytes`, `terminal_rows_rebuilt`, `terminal_runs_allocated`, and `repainted_area_px`. Correlate timestamps/pids/window IDs. One profiler owner; timers/counters **disabled in production** or aggregate cheaply under a feature switch. Prefer deterministic tests asserting counts to data-free refactors.

**Documentation:** official Cargo profile/debugging guidance: https://doc.rust-lang.org/cargo/reference/profiles.html ; Linux perf stat: https://github.com/torvalds/linux/blob/master/tools/perf/Documentation/perf-stat.txt ; Linux perf record: https://github.com/torvalds/linux/blob/master/tools/perf/Documentation/perf-record.txt. Profiling a production-equivalent release with separate debuginfo or a cloned profiling build is allowed, but NEVER compare one debug build's CPU against a release build as proof of gains.

## 6. Mandatory parallel agent execution graph (DeepSeek guidance)

**Coordinator = control-plane only** when subagents are available. First tool action should be a bounded read-only `explore` agent to freeze provenance and identify original owners; orchestrator retains only short source-fact capsules, blockers, READY DAG and verification ledger. After quick source snapshot, **saturate the READY frontier with up to five disjoint asynchronous agents**. An agent whose task ends or is blocked should be replaced immediately by a new READY disjoint task *until the verification/cleanup wave*, not by redundant scans. Do not wait for all five if independent tasks can advance. If concurrency tools are unavailable, follow the same bounded jobs sequentially rather than claim parallelism.

**Initial Wave 0 (simultaneous, read-only):**

- `E1 Desktop selection observer`: trace projected_cells/selection visuals/damage and design red test; owns no edits.
- `E2 Terminal observer`: trace PTY -> screen -> frame -> UI row mutation -> X draw and design red test; owns no edits.
- `E3 Shell/Start observer`: follow popups/desktop invalidation and Start settled CPU; owns no edits.
- `E4 WM/X11 observer`: identify what move/resize does to X server client/damage, shadows, backdrop; owns no edits.
- `E5 Profiler/build owner`: snapshot repository and run baseline **without competing Cargo jobs**; owns only benchmark logs / scoped tooling, no feature source.

**Wave 1 — READY build/edit owners once E1–E4 contracts exist:**

- `B1 Desktop owner`: **exclusively** `crates/flamewm-desktop*` (main/core) selection geometry, delta updates and tests.
- `B2 Terminal owner`: **exclusively** `crates/flamewm-terminal/**` model/row invalidation and tests.
- `B3 Shell owner`: **exclusively** `crates/flamewm-shell*` Start/control invalidation/animation tests.
- `B4 WM owner`: **exclusively** `crates/flamewm-wm*` and optionally `crates/flamewm-x11-desktop/**` native move/resize/round-trip tests.
- `B5 Shared render owner`: **exclusively** `crates/flamewm-render-core/**`, `flamewm-render-x11/**`, `flamewm-ui-x11/**` and `flamewm-reactor/**` for canonical damage/paint fixes. Shared API contract must be agreed before consumers edit; if an API migration requires touching B1–B4 files, assign the migration to the owner of those files or explicitly reassign ownership in an integration wave. Never simultaneous writers to a file.

Independent non-build analysis, tests, codegen review, and fixtures may run concurrently, but **one Cargo compiler owner at a time** (and one shared target dir). Builders do edit→focused compile/test→repair with serialized Cargo gate slot. One active Xephyr display owns interactive tests; if multiple displays are necessary use completely separate display names, processes and test directories, but do not benchmark them concurrently. Run unrelated code analysis while a build gate is busy.

**Wave 2 — integration/measurement:** independent verifier (not a builder) runs `cargo check`/full tests on a settled tree and scripted Xephyr cases; profile owner compares a *new post-integration baseline* against the saved starting measurements. Integrate only proven interface boundaries and retire the old path. **After each 3–5 meaningful optimizations**, run one full relevant workload benchmark checkpoint (not after every tiny edit), with quick deterministic tests per edit. On regression, reproduce old exact gate, repair first canonical owner, rerun identical gate. Two unsuccessful patches to same hypothesis require a discriminating new observation rather than another tweak.

**Wave 3 — saturation/reconciliation:** rank remaining opportunity by *measured attributable core-seconds × ease × confidence*. Continue cycles until 95% goal is achieved where possible, or remaining costs are demonstrably unavoidable and all testable cases reach strong documented improvements, without breaking visuals. Use no-go classification for changes that merely relocate CPU to Xephyr, degrade visual quality, or worsen latency. Finish with independent fresh verifier only.

Agent reporting schema (compact, one capsule per job):

```
ID / status: PATCH_APPLIED_NEEDS_VERIFY | BLOCKED | FAILED  (fresh verifier alone: PASS)
Files owned and changed:
Exact prior failing gate / root producer:
Change and retired bypass:
Commands, exit codes, test counts:
CPU before/after same workload (or UNMEASURED):
Input latency and rendered screenshot gate:
Known blockers / next READY task:
```

## 7. Build, testing and visual acceptance — no fake PASS

Read the live instructions and scripts; plausible starting commands:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
cargo fmt --all -- --check
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets --locked
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test --workspace --locked
# Build release binaries with EXACT profile for performance comparison:
CARGO_BUILD_JOBS=2 cargo build --release --workspace --locked
# Isolated graphical tests; script owns the nested X server and cleanup:
FLAMEWM_XEPHYR_BUILD=0 FLAMEWM_XEPHYR_CANARY=1 ./scripts/xephyr
# Final package only after source/build/visual gates are green:
./scripts/build-deb
```

If `scripts/check`, `scripts/verify-visual`, `scripts/appearance-acceptance`, source-contract Python tests and package acceptance exist, run the applicable ones; verify they do not secretly overwrite user configuration. Archive may omit runtime UI assets or fonts; never infer missing resources are project breakages without comparing live checkout. Capture screenshots for desktop selection, terminal input/selection, Start open/close, drag/resize and settings at **matching theme, accent, window opacity, blur, scale and resolution**. Ensure shadows remain bottom-right and never blacken a neighboring titlebar, maximize→minimize→restore retains maximized state, Explorer and Terminal translucency stays correct, wallpaper remains correct, and panel/time/icons never degrade. Preserve `FLAMEWM_XEPHYR_PRIVATE_DBUS=1` for normal smoke tests; wallet SDDM/PAM functionality must be tested separately on a real login by a user-owned procedure, not in nested D-Bus.

Hardware-only tests (USB eject, dual-physical-monitor mixed-scale, real login wallet) should stay accurately labeled `USER-OWNED / NOT CERTIFIED` if unavailable. **Never reset/disconnect/mount disks or edit Brave/KWallet data** in pursuit of this CPU task.

**Per-iteration acceptance:** focused tests green, compile succeeds for changed consumers, no failure masked by altered test assertions, CPU gain in same scenario, no larger allocations/memory leak, no new 100%-core idle spin, exact UI interaction and screenshot equivalence. A 95% CPU claim without per-process timing, before/after parameters and input correctness is invalid.

## 8. Crash, regressions, and diagnosis rules for DeepSeek

- On Cargo failure, isolate the first originating compiler diagnostic and actual file owner; do not pile speculative casts or changes on top. If another worker is mid-edit, wait for its change to settle and retry once, then report `EXTERNAL` with exact error if unrelated.
- On runtime repaint issue, add a failing mutation/damage test **before** patching. Explicitly distinguish `no-op event`, `changed model`, `document touched`, `layout necessary`, `repaint necessary`, `X PutImage`, `X server damage`. Optimization must eliminate unnecessary stages, not suppress visible updates.
- On keyboard lag, inspect reactor wake/deadline and PTY batching vs cursor and grapheme rendering. Avoid starving PTY events or scheduling 60fps when nothing changed.
- On `Xorg`/Xephyr CPU while idle, first identify host compositor, moving wallpaper, video/animated clients, active tool sampling and whether FlameWM/X11 owns damage; do not assume source is guilty. If isolated desktop settles at ~0 CPU, document that healthy condition and attribute production-only differences with a native session capture.
- On resize and movement, compare exact final frame geometry, snap/restore logic, intermediate position, and shadow resource lifecycle; never hide/unmap during drag to take backdrop snapshots.
- Memory/CPU tradeoff: bounded, correct caching is allowed when measured; do not implement per-surface giant image caches that multiply PSS, and don't discard icon/wallpaper caches only to cause decode CPU thrash.
- Keep readable Rust, small modules, reusable functions, and typed contracts. The simplest efficient implementation wins; no custom allocator, ECS, parallel render framework, or wholesale UI rewrite unless a demonstrated bottleneck requires it and an end-to-end migration follows immediately.
- Do not permanently add invasive profiling logs, shell polling or periodic global redraws. Shutdown/free all test X windows/PIDs created by the agent; leave the developer's primary KDE/FlameWM untouched.

## 9. Completion ledger to maintain and report

| ID | Outcome | Proof required | Initial status |
|---|---|---|---|
| C00 | Current repository/source frozen, dirty state protected | HEAD, `git status`, manifests, prior green | PENDING |
| C01 | Release CPU baseline repeatable | raw samples, methodology, median/spread | PENDING |
| C02 | CPU ownership documented | WM/desktop/shell/terminal/Xephyr/htop separated | PENDING |
| C03 | Desktop selection optimized | deltas, geometry reuse, pixel-correct GUI, CPU+damage | PENDING |
| C04 | Terminal typing optimized | PTY correctness, changed-row painting, latency/CPU | PENDING |
| C05 | Terminal text selection optimized | correct highlight/clipboard, CPU+damage | PENDING |
| C06 | Start toggle optimized | no spurious desktop paints, animations/focus correct | PENDING |
| C07 | Start settled quiet | idle after animation low absolute CPU/damage | PENDING |
| C08 | Window move optimized | proper geometry, no flicker, CPU/X server breakdown | PENDING |
| C09 | Window resize optimized | content geometry and shadow, CPU/latency | PENDING |
| C10 | Grab holds quiet | idle held pointer without motion | PENDING |
| C11 | Native desktop / X server idle quiet | production diagnosis separated from Xephyr/clients | PENDING |
| C12 | Canonical damage/layout reconciliation | no invisible stale paint, tests, screenshots | PENDING |
| C13 | Runtime memory bounded | PSS, caches and buffer lifetime | PENDING |
| C14 | Full Rust + source tests green | commands/results + no weakened tests | PENDING |
| C15 | Native Xephyr acceptance | actions, frame metrics, screenshots, no crashes | PENDING |
| C16 | Packaging/installed behavior | `.deb`, hashes, startup, expected binaries | PENDING |
| C17 | Final before/after and target evaluation | table all cases, % per role and total, 95% status | PENDING |
| C18 | No regressions / remaining blockers | protected-contract matrix, explicit USER-OWNED | PENDING |

Maintain this ledger in the actual checkout's established agent evidence area, following `AGENTS.md` / `.agents` governance; don't invent a parallel tracking framework. Include links/paths to benchmark artifacts and screenshots (actual paths only). If a task turns out to be already solved in the new live tree, prove it with a run and mark verified rather than rewriting it. User should never need to ask “continue” to get a result: keep launching READY work and publish the final outcome when the gates are exhausted.

## 10. Final instruction to the coding agent

**BEGIN EXECUTION NOW, in the live FlameWM checkout. Read the governance docs, deploy read-only explorers, establish baselines, run the five disjoint READY implementation jobs, measure real wins, fix root causes, and independently verify every visible surface and build.** Continue iterating through the highest-cost hot paths until the measurable 95% goal is met where physically possible, and make idle literally idle. Keep exact behavior, existing look, safety and architecture. If the mathematical target is not attainable without degrading features, finish the maximal verified improvement and state precisely what remains rather than fabricate success. Do not terminate at a proposal, partial code, a compile-only result, or vague claims. Deliver committed source if appropriate, measured report, graphical evidence and release package only after final gates.

---

**Research limitations:** The historical CPU samples are from CommandCode's previous Xephyr sessions, not a fresh benchmark of the newly uploaded archive; no live Xorg/perf trace was provided. A source path and suspected hot loop are **investigation priority**, not proven runtime attribution. The official profiling references explain tools and flags, but only measurements against the user's checkout establish benefits. **No source was edited by this report.**
