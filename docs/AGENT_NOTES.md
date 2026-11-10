# Agent notes: orchestrator and worker lessons

Lessons from the 2026-11-09 backlog session. Each item gives the rule, then evidence. Read with `AGENTS.md` and `docs/AGENT_PLAYBOOK.md`. Evidence paths are in this repo unless marked as session-only.

## Build and gates

1. **Gates queue on the shared cargo lock.** Parallel workers serialize on `flock /tmp/opencode/cargo.lock`, so one gate can take 10-60 min. Batch gate scope and run one gate per commit batch, not one per worker.
   Evidence: `docs/backlog/measure.md:129` (cargo under that flock; 15m 29s clean build under contention).

2. **Blocking waits only.** A worker that ends its turn mid-build gives the orchestrator a silent notification and no result. Run the build and its wait in one foreground command or one bounded wait loop.
   Evidence: session-only, reported in this session.

3. **Use the pinned 1.89 toolchain.** Clippy run under the wrong toolchain fails falsely. Set `FLAMEWM_TOOLCHAIN_BIN` to the 1.89 `cargo` directory before `scripts/gate` or `scripts/lint`.
   Evidence: `scripts/gate:10`, `scripts/lint:4-6` (CI pins Rust 1.89.0).

4. **UI changes move goldens.** Changes under `ui/` turn these red: `crates/flamewm-render-core/tests/layout_snapshot.rs` (`layout_fnv.txt`) and the `utilities.rs` digest test in `flamewm-render-compiler`. Run both crates whenever `ui/` changes.
   Evidence: `measure.md:161-163` (layout_snapshot failed; `settings_hotkeys_document_is_unchanged_by_utility_migration` failed); `look.md:125` (re-pin needed). Re-pin hash example: `look.md:79`.

5. **Known flakes: rerun alone before blaming the change.**
   - `flamewm-render-x11` lib test SIGSEGV flake.
   - `crates/flamewm-render-x11/tests/xdnd_two_windows.rs` (`xdnd_two_windows_on_private_xvfb`) flake.
   Rerun the one test or binary alone. Record both runs.
   Evidence: session-only, reported in this session; test file path verified.

## Runtime measurement

6. **Wait for two identical frames.** Captures taken during a window edge animation or a non-deterministic reveal gave false numbers. Re-reveal, then capture until two consecutive crops match.
   Evidence: `docs/backlog/dock-translucency.md:44-48` (harness waits for two identical crops); `look.md:72-73`.

7. **Check for display collisions before starting Xvfb.** Shared private displays collide. Run `ls /tmp/.X11-unix` first, pick a free `:N`, and record it in the evidence.
   Evidence: private displays already used in the backlog are `:96` (`look.md:72`) and `:97` (`dock-translucency.md:44`); the startup harness takes `:90+` (`PERFORMANCE_STARTUP_2026-11-05.md:8`).

8. **Do not trust live-data surfaces for pixel gates.** Task-manager never reached two identical frames under live process data, so it was dropped from the radius pass.
   Evidence: `docs/backlog/look.md:91-92`.

9. **Debug or contended numbers are not release numbers.** Label every timing with build profile, host load and run count.
   Evidence: `measure.md:129,186`; `wayland-rss.md:337`.

## Documentation

10. **Verify doc claims against code.** Stale claims were found only by reading code: the DND "no UI caller" note in `docs/competitors/vela/GAP_PLAN.md:77` was wrong (DND is wired, `compare.md` row 9), and README still says "No session lock" while `wm-wayland/src/session_lock.rs` exists (`README.md:89`).
   Evidence: `docs/backlog/compare.md:16`.

11. **Mark `done` only with a sha and a grep.** Check the commit is an ancestor of `origin/main`, then check the code claim with `rg`. Anything unverified stays `open`.
    Evidence: this session's `docs/BACKLOG.md` pass.

## Workflow and environment

12. **Commit trailer is exact.** Use `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Do not use the Haiku trailer. The repo rule is in `docs/AGENT_PLAYBOOK.md:50,108`.

13. **Disk fills from worker targets.** `/` filled from `/tmp/claude-1000` worker target dirs. After a worker finishes, delete its target dir and remove its worktree. Check `git worktree list` for stale entries first.
    Evidence: session-only, reported in this session. `git worktree list` showed about 60 worktrees under `/tmp/claude-1000` on 2026-11-09.

14. **Do not touch the primary tree.** Use a fresh worktree per worker. If the requested path already exists (for example `/tmp/claude-1000/wt-docs`, which held another worker's detached HEAD), pick a new path and do not reset the existing one.
    Evidence: `git worktree list` on 2026-11-09.
