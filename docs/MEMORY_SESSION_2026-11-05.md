# Session memory scout (2026-11-05)

Scope: whole-session PSS baseline on an isolated display, then two zero-code
experiments. No product code changed. Runtime numbers below are measured;
anything else is marked unmeasured.

## Setup (isolated, not :98 or :0)

- Host: `Xvfb :90 -screen 0 1920x1080x24 -nolisten tcp -noreset`
- Session display: `Xephyr :91 -screen 1920x1080 -ac -noreset -br` on `:90`.
  Xephyr is used because `tools/bench_flamewm_session_memory.py` recognises
  Xorg, Xephyr and Xwayland as the server. It does not recognise Xvfb, so a
  plain Xvfb display reports "no readable X server" and drops the server total.
- Session: `flamewm-session` with `HOME=/tmp/claude-1000/home91`,
  `XDG_RUNTIME_DIR=/tmp/claude-1000/xdg91`, `DISPLAY=:91`.
- Binaries: copied from `/run/media/linsaftw/SDD/target-deb/release` into
  `/tmp/claude-1000/pss-bin`. Provenance is not verified. The coordinator named
  commit `08e15237` (0.1.7). The worktree HEAD is `ba30f33b`. `08e15237`
  exists in the object database, but the binaries were not checked against it.
- The session must run under a long-lived parent process. Its launcher stops
  its children when the parent pid changes, so a plain `nohup ... &` dies
  when the tool call ends.
- Bench: `tools/bench_mem.sh --session --display :91 --apps 3 --launch --settle 8 --baseline none`.
  Stale app processes are killed before each run so every run measures
  exactly three fresh apps (`flamewm-terminal`, `flamewm-explorer`,
  `flamewm-settings`).

## Baseline (MALLOC_ARENA_MAX unset), 3 runs

| run | session PSS MiB | FlameWM core | apps | X server (Xephyr) | core procs |
|---|---|---|---|---|---|
| 1 | 167.1 | 44.9 | 56.3 | 65.3 | 5 |
| 2 | 165.6 | 43.7 | 56.8 | 65.0 | 4 |
| 3 | 166.7 | 43.8 | 56.8 | 66.2 | 4 |
| mean | 166.5 | 44.1 | 56.6 | 65.5 | |

Totals are partial. `flamewm-desktop-services` (the Python bridge) is absent
from the total, and the bench says so. The core process count differs between
runs (5 in run 1, 4 in runs 2 and 3). That is not explained yet, and it may
explain part of the run-to-run spread.

## Experiment 1: MALLOC_ARENA_MAX=1 (negative)

Same stack, `MALLOC_ARENA_MAX=1` exported before `flamewm-session`. 3 runs.

| run | session PSS MiB | FlameWM core | apps | X server |
|---|---|---|---|---|
| 1 | 165.0 | 44.7 | 55.8 | 64.6 |
| 2 | 165.1 | 44.6 | 54.9 | 65.5 |
| 3 | 165.4 | 44.7 | 55.2 | 65.5 |
| mean | 165.2 | 44.7 | 55.3 | 65.2 |

Delta is -1.3 MiB (-0.8%). The baseline spread is 1.5 MiB. That is inside
noise and below the 3% threshold, so arena capping is not a win here. Not
adopted. CPU was not sampled for this experiment, so it is not proven neutral,
but there is no reason to ship a change with no gain.

## Hypothesis: fontdue duplication (not resolved)

`flamewm-wm-wayland` and `flamewm-render-core` each parse a fontdue `Font`
in their own process. One sample of the WM heap was taken:

- WM `[heap]` PSS: 4.4 MiB. This is the whole heap, not just the font.
- Font share of that heap: unmeasured. Separating it needs a build with the
  font load disabled, or a heap dump.
- `flamewm-lock` was not running, so no lock-side number exists.

Upper bound on the font share in the WM is therefore 4.4 MiB. The earlier
static scout (`docs/PERFORMANCE_PSS_DUPLICATION_SCOUT_2026-11-05.md`)
ruled out a shared raw-bytes copy. It did not measure the parsed size.

## Not done

- No fix was implemented. No candidate reached the >=3% threshold on this stack.
- Shared font tables, icon-theme sharing, template/CSS caching, Vec shrinking
  and thread-stack changes were not measured.
- No perf CPU baseline. No CPU claim is made.
- `flamewm-desktop-services` was not measured. Its `.py` is not in
  `target-deb/release`, so the bridge was not launched.
- `xterm`, `xclock`, `xcalc` and `xeyes` are missing, so the apps set is
  three FlameWM apps.

## Incident

`flamewm-session --help` was run without an isolated environment. It tried to
start a session on the real `DISPLAY=:0`. The WM exited before readiness, and
the session logged to `~/.config/flamewm` and `~/.local/state/flamewm`. Those
paths may have been created or touched. Nothing was intentionally modified
and no display was shut down. Check `~/.local/state/flamewm/session.log` for
the entry from 2026-11-05 around 22:10 UTC.
