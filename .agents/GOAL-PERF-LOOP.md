# GOAL: FlameWM perpetual efficiency loop (never ends)

You are a relentless optimization machine for FlameWM: compositor/WM, explorer, task manager, terminal, shell UI. Goal: the lightest, snappiest desktop possible (CPU, GPU, compositor, RAM) with NO loss of visuals or features. Leaner code, less debt, more reliability.

## Rules
- No real tests/benchmarks of absolute numbers. Use intuition, code reading, and COMPOSITION of usage: what share of CPU/GPU/compositor time does each area take? Attack the largest slice first, then the next. Never judge by absolute CPU%.
- Read AGENTS.md and docs/ARCHITECTURE.md first. `.vendor/**` is read-only reference: check how upstream does it before inventing; shallow-clone missing upstreams into `.vendor/`.
- Orchestrate, do not hand-edit much. Use claude-opencode MCP (delegate_task / run_pool): FIRST deepseek v4.1 flash via commandcode; if its limits run out fall back to hy4-flash-free (opencode-go). Never native Agent subagents. Keep 10 parallel workers, one owner per file, disjoint units, shared checkout.
- Check get_diff; reject lazy or feature-removing work.

## Hotspot checklist (rank by composition share, rewrite the worst)
1. Idle wakeups: timers, polling loops, 60Hz tickers when nothing changed -> event-driven, coalesced, parked threads, longest-possible sleeps.
2. Render: redraw only on damage; damage regions not full frames; skip offscreen/occluded/minimized surfaces; cache blur/shadow/gradients; no per-frame allocation; batch draw calls; direct scanout for fullscreen; frame callbacks only to visible surfaces.
3. Compositor: avoid redundant layout/recompute, hash-compare before invalidating, avoid cloning big structs and strings, Arc/Cow instead of copies, bounded channels, drop lock contention.
4. Explorer: ALL filesystem work async/off the UI thread (readdir, stat, thumbnails, icons, copy/move/delete, search, size calc, watchers via inotify with debounce). Virtualized lists, incremental/streamed listing, cancelable jobs, thumbnail and icon caches with LRU and size caps, lazy metadata. UI must never block on disk, network mounts, or large moves.
5. Task manager: sample /proc incrementally, only visible rows, slow tick when hidden/unfocused, no per-row string formatting each frame.
6. Terminal: dirty-line rendering, glyph atlas reuse, batched PTY reads, scrollback ring buffer with bounded memory, no redraw when output unchanged.
7. RAM: shrink structs, interned strings, cap caches, free on idle, lazy-init subsystems and fonts/icons, avoid duplicated assets, trim dependencies and features in Cargo.toml.
8. Simplify: delete dead code, merge duplicate helpers, remove needless abstraction and unused deps, replace clever with obvious. Fix unwrap panics, races, leaks, error paths for reliability.

## The loop (compile gate every ~30 minutes)
LOOP:
 1. SCOUT: read hot code, rank areas by estimated share of CPU/GPU/compositor/RAM composition. Write the top 10-20 concrete optimizations as units in docs/perf-loop/units/.
 2. IMPLEMENT: launch up to 10 parallel workers, one per unit, refill freed slots instantly.
 3. COMMIT + PUSH after every accepted slice (git add specific files, concise message, push immediately, realtime). Never leave work uncommitted.
 4. If less than 30 minutes since the last build, return to SCOUT (next-worst hotspot; research via exa/context7/.vendor). Land at least 10-20 optimizations between builds.
 5. If 30+ minutes passed: run ONE cargo build/check (scripts per repo), collect all red errors, plan fixes together with the next batch, dispatch fixers + new optimizations, commit, push. Only then test the OS/desktop for real, and re-rank the composition.
 6. Repeat forever. There is no final state; when a pass finishes, the nearest remaining hotspot is the next unit.
## Done-ness
A unit counts only if it cuts work in a ranked hot path and keeps visuals/features. Log area, change, expected composition shift, commit in docs/perf-loop/LOG.md.
