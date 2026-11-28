# GOAL: remaster loop, simpler code, lower requirements, fewer bugs

Unattended, never completes. Never declare done or ask the user.

## Objective
Run a loop that makes FlameWM smaller, simpler, faster and less buggy. Recode whole classes, modules and architecture when that is the cleaner design (not patches), behind the existing typed facades, with ZERO visual change.
1. Lower system requirements: CPU, RAM, wakeups, startup syscalls, binary size, dependency count.
2. Deduplicate: one owner per mechanism. Find copies (rg for repeated fns, near-identical modules, duplicate deps in `cargo tree -d`), merge into the owning crate, delete the rest.
3. Simplify: remove dead code, flags nobody sets, layers that only forward calls, giant files split by responsibility, long fns split. Prefer typed state machines over bool soup.
4. Fix bugs found on the way: unwrap/expect on runtime input, unbounded caches, leaked fds/threads, races, silent errors. Each fix gets a test.
5. Reduce technical debt: stale docs, TODOs, ignored tests, duplicated scripts. Delete what is superseded.
6. Keep running on any machine: staged startup, failure records (stage, cause, blast radius, fallback), one fault test per fallback.

## Loop
discover (rg/fd/jq over crates, write one unit per area in `docs/swarm/waveN/`, each with a measurable target) -> launch OpenCode pool -> review `## Result` and `git diff` -> reject or relaunch weak units with the failure described -> batch edits -> one build/test/fmt/commit/push -> harvest `next` -> repeat. Re-rank `docs/PERF_HOTLIST.md` each round; take the largest share first.

## Rules
- Orchestrator only: Claude Code. No native subagents. Workers via OpenCode MCP `run_pool` (hy4-flash-free cap 8; check `list_models` if 500 storms; gpt-6-luna for hard units), `stale_action: abort_requeue`, brief `docs/swarm/pool/BRIEF.md`. Wait with Monitor/`wait_pool`.
- Workers: no git, no cargo, no python, no sleep loops. Re-read a file before editing; one owner per file.
- Every recode ports or cites a concrete `.vendor/` reference (path + choice) in the unit and commit.
- BUILD FREEZE (user order): no cargo build/test/clippy and no release build until a big batch is ready (at least 40 landed unit edits) or 3 hours have passed since the freeze began (froze 08:01, earliest build 11:01 machine time). Until then only edit, read, rg, fmt-free reviews and unit writing. When the build runs: fmt, test touched crates, fix the stacked errors in one round, release build, scripts/x11-resize-cpu, log in `docs/BUILD_LEDGER.md`, commit with trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`, push (the pre-push hook runs cargo fmt --check, so run `cargo fmt --all` first).
- `Cargo.toml` opt-level "z" stays uncommitted until `docs/OPT_LEVEL_PLAN.md` measures go.
- Never loosen gates. Evidence beats claims: a metric you relabeled is not proof; use CPU ticks, RSS, syscall counts, final geometry. Say so when a result is UNVERIFIED.
- Perf tests are small and scoped: one component, one second, report composition.

## Measure
Track per round in `docs/BUILD_LEDGER.md`: lines of code, crate and dependency count, release binary bytes, idle RSS, idle wakeups/s, startup statx count, resize drag CPU ticks (`scripts/x11-resize-cpu`). A round that grows any of them needs a stated reason.
