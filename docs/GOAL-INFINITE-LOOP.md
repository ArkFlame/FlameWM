# GOAL: infinite loop, make FlameWM lean, fast and unbreakable

Paste as the Claude Code goal. It never completes.

## Objective
1. Lower CPU and RAM as far as possible with ZERO visual change.
2. Start and run on every machine (X11, Wayland, missing libs, broken GPU): no hard fails; detect, report with blast radius, auto work around.

## Workers
Orchestrator is Claude Code. Never native subagents. Use the OpenCode MCP (`delegate_task` / `run_pool`, shared checkout) with hy4-flash-free on opencode-go: keep 10-20 running in parallel on different areas, refill on every finish, block with `wait_any`. gpt-6-luna or gpt-6-sol only for hard units. No python, ctx_execute scripts or sleep loops. Workers run no git and no builds or tests.

## Loop
research, investigate, fix, improve -> build/test (only if budget allows) -> if no budget, research new areas and land more edits anyway -> repeat.
- BUILD BUDGET: at most 2 builds or test runs per hour in total, batched. Keep a ledger in `docs/BUILD_LEDGER.md` (time, command, result). Spend a slot only when a meaningful batch of edits is waiting. No slot: never idle, never poll; keep researching, editing and queuing units. Only the orchestrator builds, then fixes stacked errors in one round.
- LAND REALTIME: every fix is committed and pushed at once (small commits, `cargo fmt` first: the pre-push hook checks it). Unbuilt edits say "unbuilt" in the message and are verified in the next slot.

## Phase 0: benchmark ONCE, never again
`scripts/bench-baseline` (bash or Rust, reusable), scoped per `.agents/skills/scoped-perf-tests/SKILL.md`: one component, one second of use, share per subsystem. Cover idle compositor, move/resize, start menu, popups, clock tick, notifications, icons, lock, terminal, settings, per-process RSS and wakeups/s. Freeze results in `docs/PERF_BASELINE.md`. Never re-benchmark the whole system; later work runs only the small scoped test of what it changed.

## Phase 1: ranked contributors
`docs/PERF_HOTLIST.md`: every CPU, RAM and wakeup contributor sorted by share: area, share, cause, planned cut, status. Re-rank after each cut.

## Phase 2: cut, forever
Take the hottest row: zero idle wakeups (event driven, coalesced timers), reuse allocations, lazy-load, shrink embedded assets, damage-only repaint, shared buffers. Visuals stay pixel-identical: compare frames from `scripts/shots/shot.mjs` or `scripts/look-capture` and READ them. Gates never loosen. When the list is empty, research new areas and add rows. Never declare done.

## Phase 3: reliability and fail-safe
Staged startup with error codes and a failure record (stage, cause, blast radius: what works, what degraded). Automatic fallbacks: backend DRM, nested, X11, minimal mode; GPU hardware, software, CPU raster; missing lib, font, icon, theme or portal: bundled fallback or skip the feature, never abort; corrupt settings: quarantine, load defaults, tell the user once; panic: catch at subsystem boundary, restart it, keep the session, report to the session log. Each fallback gets a fault-injection test. Start from the real laptop logs of the X11 and Wayland no-start failures: reproduce, fix the root cause, then add the fallback.

## Reference-first
Read competitors in `.vendor/` before inventing: kwin-master, xfwm4-master, icewm-master, cosmic-epoch, smithay-master, mangowm, starling-main, vela-main, mate-desktop-master, xfdesktop-master, sddm-develop, lightdm-main. Port proven algorithms; record the precedent (path, choice) in the commit message.

## Rules
Judge by the running desktop and frames you read, not green scripts. A failed attempt means change method. One owner per file; re-read before edit. Board line per unit: status, files, scoped before/after, frame path, next units. After every batch write the next hottest and least reliable areas as units and launch them.

## Cadence (user order 2026-11-27)
- Run a MASSIVE number of OpenCode workers (pool as full as the provider allows) on fixes and improvements; land their edits in batches, never one by one.
- Build/test windows are 30 to 60 minutes apart at minimum (never every 5 minutes), still within the 2 per hour cap.
- BUILD TIME: stop launching workers, wait for every running worker to finish (wait_any), then build/test, fix stacked errors, and only then resume launching.
