# z08 — live X11 harness remaster (design + step 1)

Scope: `scripts/x11-startup-faults` (live X11 startup-fault harness) and its index
`docs/FAULT_TESTS.md:24`. No build, no cargo, no git, no network, no host display, no new deps.
Visuals are not involved: only shell `PASS`/`FAIL`/`SUMMARY` text is produced, so the change is
pixel-identical by construction.

## Current structure (file:line, read from the shared checkout)

| Lines | Owner | Role |
| --- | --- | --- |
| `scripts/x11-startup-faults:1-15` | header | four-case contract, exit codes, no-build claim |
| `:16-27` | preflight | `set -uo pipefail`, `BIN_DIR`/`WM`, `XVFB`, `timeout` checks |
| `:29-33` | `WM_LAUNCH` | optional `dbus-run-session` wrapper |
| `:35-57` | sandbox | `mktemp -d` base, `WMPID`/`XPID`, `trap cleanup EXIT` |
| `:59-60` | report | `report_pass`/`report_fail`, one line per case |
| `:62-72` | `case_dir` | private HOME/XDG per case |
| `:74-81` | `assert_failure_line` | stage + blast_radius on the same classified line |
| `:83-99` | `start_xvfb` | display pick `:86-87`, `DISPLAY` export `:88`, launch `:89-90`, readiness `:93-96` |
| `:101-111` | case 1 | `display-unset` → connect |
| `:113-127` | case 2 | `display-97-no-server` → connect |
| `:129-151` | case 3 | `second-wm-on-private-xvfb` → become-wm; fixed `sleep 2` at `:135` |
| `:153-167` | case 4 | `healthy-wm-3s` → timeout 124/137, no fault line |
| `:169-170` | tail | `SUMMARY cases=4 failed=<n>` and aggregate exit |

Local conventions consulted: `scripts/break/lib.sh:23-29` (`pick_display` checks socket **and**
lock), `scripts/look-capture:25-28` (private Xvfb, socket-only readiness),
`scripts/break/break.sh:7` and `scripts/dev/env.sh:1` (sibling lib sourcing).

## Counted problems

1. **P1 — socket-only free check** (`scripts/x11-startup-faults:87`): a stale `/tmp/.X<n>-lock`
   with no socket is read as free, so Xvfb can fail to bind. `scripts/break/lib.sh:26` checks both.
2. **P2 — unpinned display ownership** (`:86-88`): the pick and the Xvfb launch are separate with
   no lock; two concurrent harness instances can choose the same display.
3. **P3 — no typed display string** (`:88`, `:117`): `DISPLAY` is concatenated by hand in two
   places, so the display value has no single owner.
4. **P4 — blind readiness loop** (`:93-96`): polls only the socket, never `kill -0 "$XPID"`, so an
   Xvfb that dies immediately burns the full 5 s and reports a misleading "not ready".
5. **P5 — five duplicated launch expressions** (`:105`, `:120`, `:133`, `:139`, `:157`): the
   `timeout --signal=TERM --kill-after=2s <dur>s "${WM_LAUNCH[@]}"` form is repeated; no typed case
   runner owns it.
6. **P6 — fixed first-WM delay** (`:135`): `sleep 2` does not prove the first WM claimed the root,
   so the second WM can race and fail at `connect` instead of `become-wm` (flaky assertion).

## Target design

- `scripts/x11-live-facade.sh` (new, source-only): the typed facade for this harness.
  - `x11_live_display_name NUM` → canonical `:NUM` (single owner of the display string).
  - `x11_live_pick_display FIRST LAST HINT` → first free number, checking both socket and lock
    file; roots are `X11_LIVE_SOCKET_DIR` / `X11_LIVE_LOCK_DIR` so tests are hermetic.
- `scripts/x11-startup-faults`: sources the facade, routes `start_xvfb` through it; later steps
  add liveness-aware readiness and one `run_wm_case` runner, keeping one `PASS`/`FAIL` line per case.
- `scripts/test-x11-live-harness` (new, test-only): pure-shell tests for the facade; no X server,
  no build, no network. Prints `PASS`/`FAIL` per test plus `SUMMARY tests=<n> failed=<n>`.

Non-goals: no rendering or visual code, no dependency additions, no changes to the fault-case
assertions or their expected strings, no edits to `.vendor/**`.

## Steps (each one compiles alone)

"Compiles alone" for a shell unit means: `bash -n` is clean and `scripts/x11-startup-faults` stays
runnable after that step only.

1. **Step 1 — typed display facade (implemented here, UNBUILT).** Add
   `scripts/x11-live-facade.sh` with `x11_live_display_name` and `x11_live_pick_display`; source it
   from `scripts/x11-startup-faults` and replace the inline display pick/export in `start_xvfb`
   (`:86-88`) with facade calls; add `scripts/test-x11-live-harness`. P1, P2 (partial: lock check +
   hint-based scan) and P3 are addressed. `bash -n` clean alone; no launch-site or assertion change.
2. **Step 2 — liveness-aware readiness.** Inside the `:93-96` loop, fail fast when `kill -0 "$XPID"`
   is false and keep the bounded deadline. P4. Independent of step 1; `bash -n` clean alone.
3. **Step 3 — one case runner.** Add `run_wm_case dir label duration` to the facade and route the
   five sites (`:105`, `:120`, `:133`, `:139`, `:157`) through it without changing durations,
   signals, exit-code checks or printed lines. P5. `bash -n` clean alone.
4. **Step 4 — root-ownership probe.** Replace the fixed `sleep 2` at `:135` with a bounded probe
   that the first WM owns the root before launching the second. P6. `bash -n` clean alone.

## Rollback

Each step is a bounded addition to a named file, so rollback is a single-file reversal:
- Step 1: delete `scripts/x11-live-facade.sh` and `scripts/test-x11-live-harness`, remove the
  `source` line from `scripts/x11-startup-faults`, and restore the three inline lines at `:86-88`
  (`num=$((600 + ($$ % 300)))`; socket-only `while`; `export DISPLAY=":$num"`).
- Step 2/3/4: revert that step's hunk only; earlier steps remain intact.
No config, dependency, lockfile or visual artifact is created, so no cache cleanup is needed.

## Reference port (path + choice)

Question: who owns the canonical X display string and how is a private display chosen for a WM
harness? Active owner: this shell harness only; no product crate or WM authority is changed.

Reference: `.vendor/xfwm4-master/common/xfwm-common.c:191-224`
(`substitute_screen_number` → `xfwm_make_display_name`): one typed helper derives the canonical
display name from a display + screen number and strips any prior suffix; callers never build it by
concatenation. A file-name scan of `.vendor/xfwm4-master` (1236 files) found **no test harness**, so
there is no vendor test to port — the ported item is the *single-helper display-name contract*, not
code. Choice: reimplement it as `x11_live_display_name` in the facade and pair it with the local
free-display precedent `scripts/break/lib.sh:23-29` (socket **and** lock file), adding hermetic
root overrides for tests. No vendor source is copied, linked, loaded or used at runtime.

Validation: `bash -n` on the harness, facade and test; the test script is UNBUILT (no cargo build,
no X server) and is left for the orchestrator.
