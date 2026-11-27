# GOAL: infinite loop, make FlameWM lean, fast and unbreakable

Paste as the Claude Code goal. It never completes.

## Objective
1. Lower CPU and RAM of the desktop as far as possible with ZERO visual change.
2. Start and run on every machine (X11, Wayland, missing libs, broken GPU): no hard fails; detect, report with blast radius, auto work around.

## Workers
Orchestrator is Claude Code. Never spawn native subagents (they drain quota). Use the OpenCode MCP: `delegate_task` / `run_pool` with model hy4-flash-free on opencode-go, shared checkout. Keep the pool full (dozens), refill on every finish, `wait_any` to block (no sleep loops, no python, no ctx_execute scripts). Medium or hard units: gpt-6-luna or gpt-6-sol. Workers run no git or full builds; the orchestrator builds once per batch.

## Phase 0: benchmark ONCE, never again
- Write `scripts/bench-baseline` (bash or Rust, reusable). Scoped per `.agents/skills/scoped-perf-tests/SKILL.md`: one component, one second of simulated use, report share per subsystem, not absolute CPU.
- Cover: idle compositor, move/resize, start menu, popups, clock tick, notifications, icons, lock, terminal, settings, per-process RSS and wakeups/s.
- Save to `docs/PERF_BASELINE.md` (date, commit, machine, raw numbers). It is frozen. Never re-benchmark the whole system; later passes only run the small scoped test of the one thing they changed and compare to the frozen row.

## Phase 1: ranked contributors
Build `docs/PERF_HOTLIST.md`: every CPU, RAM and wakeup contributor, sorted by share, one row each: area, share, cause, planned cut, status. Re-rank after each cut.

## Phase 2: cut, forever
Take the hottest row. Cut it: idle wakeups to zero (event driven, coalesced timers), cache and reuse allocations, lazy-load, shrink embedded assets and tiers, drop per-frame work, damage-only repaint, share buffers. Visuals must stay pixel-identical: capture before and after with `scripts/shots/shot.mjs` or `scripts/look-capture`, READ both frames, reject any difference. Gates never loosen. Then next row. When the list is exhausted, research new areas (profile one scoped component, read the code, find unmeasured cost) and add rows. Never declare done.

## Phase 3: reliability and fail-safe
Build a robust layer: startup stages with explicit error codes, a failure-report record (stage, cause, blast radius: what still works, what degraded), and an automatic fallback chain per stage:
- backend: DRM, then nested/winit, then X11, then headless-safe minimal mode.
- GPU: hardware, then software render, then plain CPU raster.
- missing library, font, icon, theme, config or portal: bundled fallback or skip the feature, never abort.
- corrupt or unreadable settings: quarantine the file, load defaults, tell the user once.
- crash or panic: catch at subsystem boundary, restart the subsystem, keep the session alive, write a report under the session log.
Each fallback gets a fault-injection test asserting the session still starts. Collect the laptop failure modes (X11 and Wayland no-start) from logs first, reproduce, then fix the root cause, not only the fallback.

## Reference-first
Before inventing, read how competitors do it in `.vendor/`: kwin-master, xfwm4-master, icewm-master, cosmic-epoch, smithay-master, mangowm, starling-main, vela-main, mate-desktop-master, xfdesktop-master, sddm and lightdm for session startup. Port proven algorithms, record the precedent (path and choice) in the commit message or docs. Clone more into `.vendor/` shallow if needed.

## Rules
- Judge by the running desktop and frames you read, not green scripts.
- Failed attempt: change method, never stop.
- Existing `AGENTS.md` and `.opencode` governance apply. One owner per file; re-read before edit.
- Commit and push each verified slice. Log one board line per unit: status, files, scoped measurement before and after, frame path, next units.
- After every batch, write the next hottest and least reliable areas as units and launch them.
