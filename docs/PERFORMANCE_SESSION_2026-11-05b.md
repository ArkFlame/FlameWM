# Performance session 2026-11-05b: interaction CPU on the X11 session

Status: measurement only. No code changed. Hot spot located, fix not applied.

## Method

- Binaries: `/run/media/linsaftw/SDD/target-deb/release` (0.1.7, commit 08e15237). The current tree did not build (rustc crashed on `resvg` 0.48.1 at opt-level 2 under host load).
- Harness: `x11_bursts.py` (python-xlib XTest, per-pid `/proc/<pid>/stat` utime+stime deltas, grouped by comm, pids alive in both samples only). Private Xvfb `:96` and a private session bus. Launched `flamewm` (WM), `flamewm-shell`, `flamewm-desktop`, `flamewm-terminal`, `flamewm-explorer` (5000-file dir).
- Host load was 17 to 23 on 12 cores during runs. CPU-seconds are used, not wall percent.
- Baseline: 3 runs x 10 s per burst (`/run/media/linsaftw/SDD/meas-flamewm/x11-baseline.json`, `x11-baseline.log`).

## Baseline CPU-s per 10 s burst (3 runs, same binaries)

| Burst | flamewm-terminal | Xvfb | flamewm-explorer | flamewm-desktop | others |
|---|---|---|---|---|---|
| idle | 0.00 | 0.00 | 0.00 | 0.00 | <0.03 |
| drag (press at frame+8 px, moves) | 9.51 to 9.63 | 2.27 to 2.41 | 0 | 0 | flamewm <0.02 |
| resize (same press point) | 9.56 to 9.60 | 2.36 to 2.43 | 0 | 0 | flamewm <0.02 |
| taskbar hover sweep | 0.00 | 0.11 to 0.12 | 0 | 0.12 to 0.16 | |
| menu open/close x50 (right-click, Esc) | 0.04 to 0.05 | 0.33 to 0.38 | 0.47 to 0.54 | 0 | |
| terminal `yes`-style typing | 0.10 to 0.11 | 0.39 to 0.42 | 0.32 to 0.36 | 0 | dbus 0.05 |
| explorer wheel scroll | 0 | 0.59 to 0.68 | 0 | 0 | flamewm 0.09 |

Ranking by CPU-s per burst: drag/resize on the terminal (about 96% of one core) far exceed everything else (the next largest is about 0.7 s per 10 s, from Xvfb or explorer).

Not run: workspace switch x50 (no key binding was confirmed), `yes | head -200000` as a separate burst (typed only).

## Split: what the terminal burns

| Burst (1 run, 5 s) | flamewm-terminal CPU-s |
|---|---|
| titlebar drag (press on WM titlebar, move) | 4.42 |
| body selection drag (press in client, move) | 4.73 |
| plain pointer motion over the terminal body, no button | 0.09 |
| idle | 0.00 |

Any held-button motion costs about 90% of a core in the terminal. Plain motion costs about 2%.

## Root cause (static, not yet profiled)

`crates/flamewm-render-x11/src/app/events.rs:574-615`: while a pointer button is held and an interaction node is active, each MotionNotify takes the `fire_node(...) || changed` branch and calls `mark_input_repaint()` (line 592-593 region). `redraw_if_dirty` (`render.rs:1125-1150`) then repaints every time, with no frame throttle. The loop coalesces only contiguous queued motions (events.rs:507-546). The harness sends motion every about 8 ms, so this repaints about 120 times a second, and each repaint is a full software scene rebuild.

The hover branch (no button) repaints only when `changed` is true, which is why plain motion is cheap.

## Candidate fixes (not applied)

1. Frame-cap held-button motion repaints at 16 ms (`FRAME_INTERVAL` as in `flamewm-reactor/src/timeline.rs:15`). This needs a deferred flush so the last motion before a pause is still painted. The current loop has no timer for that.
2. Gate the held-button branch on `changed` the way the hover branch does. This changes the slider and radial semantics, since `fire_node` returns "handled", not "changed". Needs review of every node that relies on the repaint.

Neither was implemented in this session. Both need a before/after run with this harness and a regression test in `flamewm-render-x11`.

## Negative results

- Wayland nested session (`fm.sh hover`/`marquee`): flamewm-wayland 5.5k wakes/s, 68 to 78% CPU. Mesa llvmpipe worker threads, a known full-repaint artefact (see `PERFORMANCE_SETTINGS_WAKES_2026-11-05.md:22,44`). Not pursued; the full repaint is the nested-winit BUFFER_AGE limit and `damage_render` is not touched.
- Idle (`fm.sh idle`, 3 runs): 3.95, 21.97 (outlier), 3.97 wakes/s total, CPU 0.00 to 0.24%. The 18 w/s outlier on flamewm-wayland in run 2 was not explained. Not reproduced.
- The three sleep sites from the prior scout are dismissed: `x11-desktop` lib.rs 697/718 are test-only; `logind` `wait_for_active` runs once at activation; `backdrop` recapture runs only during drag bursts.
- Menu open/close, taskbar hover, typing, explorer wheel: each under 1% of one core in these runs. No fix needed.

## Harness

`scratchpad/x11_bursts.py` (not committed). Modes: `idle`, `drag`, `resize`, `drag_titlebar`, `select_body`, `hover_body`, `taskbar_hover`, `menu_x50`, `terminal_yes`, `explorer_scroll`. Usage: `python3 x11_bursts.py <bindir> <runs> <secs> <out.json>`, with `BURSTS=` to select modes.

## Open items

- Confirm the held-button repaint with a profile or a counter of `redraw_if_dirty` calls per second. `perf` is blocked (`perf_event_paranoid=4`).
- Implement candidate 1 or 2, then repeat the 3 x 10 s drag and resize runs.
- Workspace switch and a separate `yes | head -200000` burst still need to be run.

## Held-motion repaint coalescing (option 1): measured NEGATIVE

Change: held-button motion repaints capped at one per 16 ms via reactor deadline (`held_motion_paint_gate`, 3 unit tests pass, `test_exit=0`). Release build `build_exit=0` (target-s4).

flamewm-terminal CPU-s per 10 s burst, 3 runs:

| burst | before (target-deb) | after (target-s4) |
|---|---|---|
| drag | 9.51, 9.63, 9.51 (baseline) | 9.50, 9.48, 9.43 |
| resize | 9.56, 9.56, 9.60 (baseline) | 9.45, 9.45, 9.41 |
| select_body | 9.52, 9.57, 9.52 | 9.47, 9.49, 9.45 |

Result: no measurable change (under 1%). The coalescing gate sits in the wm-x11 render loop; the ~95% core cost is in flamewm-terminal's own loop, which this change does not touch. Not committed as a fix. Source: `/run/media/linsaftw/SDD/meas-flamewm/x11-after.json`, `x11-before-select.json`, `x11-baseline.json`.

Open: terminal's own motion/paint path (crates/flamewm-terminal/src/app/pointer.rs) is the next target; not yet profiled per thread.

## flamewm-terminal drag cost: where it is NOT

- Thread split (`/proc/<tid>/stat`, `prof/threads.txt`): the main thread holds all CPU. Worker threads show 0 ticks. The sampler attached late, so this shows the tail of the burst only.
- Per-event projection is not the cost. A temporary counter in `RunCache::update` (`crates/flamewm-terminal/src/view.rs`) recorded 2 updates and 64 rebuilt rows over a 10 s drag burst. The probe is removed; `view.rs` is unchanged.
- `ui.rs` projection and `view.rs` row cache already diff and skip unchanged rows. `pointer.rs` motion handling is a few comparisons plus at most one selection write.
- Stack sampling is blocked: `gdb -p` gets "Could not attach" (yama ptrace_scope). `strace -f -c` over the harness tree did not finish: the harness children stayed alive under strace, so the summary was never written. `FLAMEWM_PROFILE=1` wrote no span log for the terminal.
- Conclusion: the ~9.5 CPU-s per burst is on the main thread, outside `RunCache`, `project`, and `pointer`. The hot spot is not yet identified. Candidates to check next: the per-motion event path in flamewm-ui-x11 / flamewm-render-x11 (hit-test, hover, window backdrop or blur work per input packet), and the event loop's spin on queued input.
- No terminal fix was made. The render-x11 coalescing change was dropped because it cannot reduce this cost.
- Infra: the harness leaves per-run `dbus-daemon` and app processes behind when a run is killed mid-way (`/tmp/flamewm-bursts.*`). Those were cleaned up for this session's runs.
