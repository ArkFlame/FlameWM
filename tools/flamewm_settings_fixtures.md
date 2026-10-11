# System Settings native acceptance: fixtures and scenario matrix

Runner: `tools/flamewm_settings_acceptance.py` (wrapper: `scripts/settings-acceptance`).
Schema: `flamewm-settings-acceptance/2` (`schema_version` 2). Each run writes `<run>/result.json`,
`<run>/shots/*.png`, `<run>/logs/*.log`. Nothing outside `<run>` is written by the runner.

## Result model

- Statuses are exactly four and distinct: `PASS`, `FAIL`, `NOT_RUN`, `ENVIRONMENT_BLOCKED`.
  - `NOT_RUN`: no runner implementation exists for the scenario, or only partial coverage was measured.
  - `ENVIRONMENT_BLOCKED`: the runner tried, and this host could not measure it.
  - `FAIL`: a measured check failed. A FAIL on any canonical or legacy entry makes the verdict FAIL.
- `scenarios`: the 20 canonical entries SS00..SS19, always in order, with canonical labels.
- `legacy_subscenarios`: the runner's own sub-scenarios, reported as `LEGACY_HARNESS_<ID>`.
  They never count as a canonical PASS on their own.
- `verdict`: `FAIL`, `INCOMPLETE`, or `ALL_SCENARIOS_PASS`. The runner never emits `READY`.
- `exit_code` / process exit: `0` means no FAIL. It is not readiness. `1` means a FAIL. `2` is
  a usage error. `3` means the run could not start. Readiness is decided by the release gates
  (SS16-SS18) and by reading `result.json`.

## Canonical matrix (SS00..SS19)

| ID | Canonical label | Runner status |
|---|---|---|
| SS00 | source/git preservation | implemented (git status before/after) |
| SS01 | seven pages open | implemented (7 pages at the base screen size) |
| SS02 | seven pages x 760x500/820x540/960x640 | implemented; FAIL unless all 21 captures are live/nonblank and measured client geometry exactly matches |
| SS03 | accent | NOT_RUN (no runner) |
| SS04 | icon theme | typed cycle/unknown snapshot plus a real GTK3 client (owned display) reading XSETTINGS `gtk-icon-theme-name` and `folder` icon lookup; unrelated-setting control; fresh client after an owned WM restart. Shell does not read `iconTheme`, XSETTINGS is the only consumer. PASS needs the client to report each requested theme |
| SS05 | wallpaper | NOT_RUN (chooser is interactive; no non-interactive path on this host) |
| SS06 | sticky notes/opacities | typed snapshot, WM restart persistence, and real `flamewm-desktop` captures from private-HOME fixtures (one Desktop file, one sticky note): note pixel hidden after `stickyNotesEnabled=false`, selection fill red level at 20/21/50 percent, unrelated-setting and unchanged-capture controls. NOT_RUN overall: `flamewm_xtest.py` has no drag primitive, so the WM snap-preview fill (`snap_preview_consumer`) cannot be driven |
| SS07 | taskbar | Settings and Panels snapshots plus real `flamewm-shell` captures: taskbar extent, color x opacity pixel, edge move, unchanged-capture and unrelated-setting controls, and a freshly started Shell after an owned WM restart. Panel edge is reported (not gated) as persisted or reset |
| SS08 | displays 1/2/4/5+ outputs and modes | NOT_RUN; partial: `LEGACY_HARNESS_NATIVE_ONE_OUTPUT` covers only the 1-output case |
| SS09 | 15s rollback | NOT_RUN (a real 15 s revert cannot run in Xephyr; needs a fake-clock test) |
| SS10 | scale | native runner (`run_ss10`); needs `--ss10-scale-click`, plus `--ss10-wheel-steps` and `--ss10-scrolled-click` when the output lists modes. PASS only when the Displays 125 click changes the output `scale` to 125, the mode list is unchanged, and the value persists after an owned WM restart; otherwise NOT_RUN |
| SS11 | fonts | native runner (`run_ss11`); needs `--ss11-family-click` and `--ss11-size-click`. PASS only when both clicks change `fontFamily` / `fontSizeOffset` in the Settings snapshot and both persist after an owned WM restart; otherwise NOT_RUN |
| SS12 | hotkeys | native runner; PASS only when lease, stored binding, action, grab restoration, owner loss, Escape clear (empty binding, persisted after restart) and Reset (`Super_L`, persisted after restart) all pass; Start is observed as the `StartMenuToggleRequested` D-Bus signal (no Start window exists in the session); clicks default to measured screen points 593,180 / 733,180, valid only for the 820x540 client at (0,31) (otherwise refused NOT_RUN); `--ss12-click` and `--ss12-reset-click` override |
| SS13 | about | native runner (`run_ss13`); PASS only when About renders non-blank at 760x500, 820x540 and 960x640 and the Rust test `about_version_text_is_exact_flamewm_version` passes via `cargo test`; NOT_RUN when cargo is not on PATH |
| SS14 | external signal + auto-revert | Settings accent, `Panels.SetEdge` on the open Taskbar page, `Shortcuts.SetBinding` on the open Hotkeys page (each with unchanged-capture and unrelated-setting controls, no input), and an owned WM restart under an open Settings client followed by an external accent Apply. Display rollback stays NOT_RUN: Xephyr exposes no alternative modes, so no revert transaction can start |
| SS15 | traversal/disabled states | NOT_RUN (no runner) |
| SS16 | cargo/CSS/architecture gates | runner executes `static_audit`, `architecture_guard`, `verify_rwr_promotion`, `test_j18_frame_guard.py`; PASS only when all exit 0 on one source (cargo gates are not run here) |
| SS17 | private XTEST+screenshot matrix | NOT_RUN (no runner) |
| SS18 | installed-prefix release | maps `--ss18-installed-result installed-result.json`; PASS only when the staged-binary run reports launch PASS plus SS03/SS05/SS11/SS12 PASS; launch smoke alone stays NOT_RUN |
| SS19 | stability/perf/PID cleanup | NOT_RUN; partial: `LEGACY_HARNESS_NATIVE_PID_CLEANUP` |

## Legacy sub-scenarios (report labels)

| Legacy ID (report label) | Old label | Canonical ID it partially covers |
|---|---|---|
| `NATIVE_ONE_OUTPUT` (`LEGACY_HARNESS_NATIVE_ONE_OUTPUT`) | Displays page with exactly one output (`xrandr --query` counts one connected output) | SS08 |
| `NATIVE_HOTKEY_INPUT` (`LEGACY_HARNESS_NATIVE_HOTKEY_INPUT`) | Hotkey capture via XTEST keys | SS12 |
| `NATIVE_PID_CLEANUP` (`LEGACY_HARNESS_NATIVE_PID_CLEANUP`) | Runner teardown: every PID it started is gone | SS19 |
| `NATIVE_PERSISTENCE_RESTART` (`LEGACY_HARNESS_NATIVE_PERSISTENCE_RESTART`) | Persistence and restart under private XDG directories | none |

The old labels "Displays one output" (for SS03), "hotkey" (for SS06) and "PID cleanup" (for SS19)
are retired. `tools/test_flamewm_settings_acceptance_schema.py` fails if those canonical IDs
are relabeled.

## Isolation contract

- Private run directory, created fresh (refused if it exists non-empty, or lives under
  `.debug`, `.performance`, `.target` or `.agents/runtime`).
- Private `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_RUNTIME_DIR`
  (mode 0700) under `<run>`.
- Private D-Bus: the runner re-execs itself once under `dbus-run-session --`.
- Owned Xephyr on the first free display in `:97`..`:99`, then `:90`..`:96`
  (`:97`..`:119` is the only range `tools/flamewm_xtest.py` accepts). Explicit `--display`
  must be in `:90`..`:99` and is refused when in use. Xephyr is started with `-ac`; this is a
  local nested server only.
- Only PIDs the runner started are signalled (SIGTERM, then SIGKILL after 5 s). There is no
  `pkill`/`killall`. `NATIVE_PID_CLEANUP` verifies that none of the recorded PIDs is still running.
- XTEST input is delegated to `tools/flamewm_xtest.py` by CLI. Its owner token is written
  to `<run>/xephyr-owner` (mode 0600) and redacted as `<owner-token>` in `result.json`.

## Window size

- The Xephyr screen is `--geometry` (default `1280x800`). `MIN_SCREEN` is the largest matrix size
  (`960x640`) and gates only the screen, not the smaller sizes.
- SS02 launches each page with `--window-size WxH`, then measures the native Settings client using
  the named client in `xwininfo -root -tree`. This deliberately skips the WM's undecorated frame,
  whose height includes the titlebar. Every requested size must have seven live clients, seven
  nonblank captures and exact measured client width/height. A missing option, absent window, blank
  capture or size mismatch is a product/harness FAIL; Xephyr screen dimensions alone are never
  accepted as evidence.

## Input tool: `tools/flamewm_xtest.py`

- `--click X,Y`, `--key KEYSYM` and `--chord MOD+...+KEY`: press and release (immediate).
- `--key-down KEYSYM` and `--key-up KEYSYM`: press and leave held, then release in a later call.
  The XTEST key state lives in the nested server, so the pair can span two invocations.
- `--hold KEYSYM:MS`: press, hold for MS milliseconds (1..60000), then release in one call.
- `--wheel up|down[:N]`: N (1..100, default 1) wheel button 4 or 5 press/release pairs at the current pointer
  position; combine with `--move X,Y` in the same call to scroll over a point.
- Each call needs the owner token and marker from the owning Xephyr. Unit tests use a fake backend
  (no X): `tools/test_flamewm_settings_acceptance_schema.py`.
- SS12 `--ss12-click X,Y` targets the `ToggleStartMenu` hotkey row; `--ss12-chord` defaults to
  `Super+F8`. The runner records Begin/End D-Bus call senders, checks the authoritative shortcut
  snapshot, injects the chord using one XTEST connection, replays it after capture, and tests owner
  loss by exiting Settings during a second active capture. Escape clear and Reset are checked in the
  Shortcuts snapshot and again after an owned WM restart; Reset must return `ToggleStartMenu` to
  `Super_L` (the platform default). Start behavior is observed by native window title; when no Start-like
  window exists in any observation (WM only, no shell process) those two observations are unobserved (NOT_RUN), not failed.
- SS10 `--ss10-scale-click X,Y` is a client-relative click on the 125 scale button of the Displays page
  in a 960x640 client (default `751,553`; at 820x540 the scale row is below the fold). With zero listed
  modes it is clicked directly. Each listed mode pushes the scale row below the fold, so when the first
  authoritative output lists modes the runner wheel-scrolls first: `--ss10-wheel-steps N` (1..100, wheel
  down, pointer at the client centre `480,320`, sent as `flamewm_xtest.py --move ... --wheel down:N`), then
  clicks `--ss10-scrolled-click X,Y`. Both are required on that path; without them, or when the wheel
  input exits non-zero, SS10 is NOT_RUN and nothing is clicked. No scrolled click point is measured or
  defaulted. The wire key is `scale` (Rust field `shell_scale_percent`). The runner requires a different
  starting value, clicks, then requires exactly 125 and an unchanged mode count in the Displays snapshot.
  It then restarts the owned WM and requires 125 again. A click with a non-zero exit, any other value,
  or a changed mode list fails.
- SS11 `--ss11-family-click X,Y` and `--ss11-size-click X,Y` are client-relative clicks on the Fonts
  page in an 820x540 client. The family click must change `fontFamily`; the size + click must raise
  `fontSizeOffset` by exactly one with the family unchanged. Both values must persist after an owned
  WM restart. Defaults `697,150` and `760,254` were measured from pixels at 820x540.
- SS13 opens `flamewm-settings --page about` at each matrix size and requires an exact measured client
  size and a non-blank capture. The exact About text (`Version <FLAMEWM_VERSION>`) is verified by the Rust
  test `about_version_text_is_exact_flamewm_version` in `crates/flamewm-settings/src/controller.rs`. The
  runner executes `cargo test -p flamewm-settings about_version_text_is_exact_flamewm_version` from the repo
  root with `CARGO_BUILD_JOBS=2`, only when `cargo` is on PATH. PASS needs exit 0 and the line
  `test result: ok. 1 passed`; any other outcome is a measured FAIL. Without cargo the check is unobserved
  (NOT_RUN, never PASS), with the three renders recorded as evidence. The command, exit code and result
  line are in `details.version_test`. The runner reads no Settings snapshot version key. No OCR is used.
- SS14 `--ss14-accent-pixel X,Y` samples a pixel inside the Appearance page's `nav-selection-accent`
  bar. It sends one `Settings.Apply(accent)` from a separate `gdbus` client, records the Apply call
  and both Settings change signals, reads the authoritative revision/value, then samples the same
  pixel after waiting without mouse/key input. This only exercises the Settings accent signal path;
  panels, shortcuts, display rollback, and signal-loss recovery remain explicitly unverified.
- SS04 cycles the three supported icon theme identifiers through private Settings state, checks the
  unknown-theme value round trip, then restores the prior value. The runner does not launch
  `flamewm-shell`, so cache invalidation, real installed-theme fallback, and menu/desktop rendering
  stay unverified.
- SS06 toggles sticky-note enablement and changes both opacity values (using a distinct in-range
  value from the authoritative `0..60` range) in one private Settings transaction, checks snapshots
  across an owned WM restart, and leaves desktop-scene visuals unverified because `flamewm-desktop`
  is not part of this runner session.
- SS07 changes global Taskbar height/opacity/color and one private Panel edge, then checks the
  Settings and `Panels.GetSnapshot` values for the private connected output. It does not run the
  Shell renderer or verify restart equivalence, so canonical SS07 remains `NOT_RUN`.

## Verifier commands

```
python3 -I tools/test_flamewm_settings_acceptance_schema.py
python3 -I -m py_compile tools/flamewm_settings_acceptance.py tools/flamewm_xtest.py
scripts/settings-acceptance --run-dir <fresh-empty-dir>
scripts/settings-acceptance --run-dir <fresh-empty-dir> --geometry 1280x800 --display :97
scripts/settings-acceptance --run-dir <fresh-empty-dir> --hotkey-click 400,300 --hotkey-key F2
```

The live runner needs a built `.target/debug/flamewm` and `.target/debug/flamewm-settings`
(or `--wm-bin` and `--settings-bin`). Read `<run>/result.json` and the shots; do not rely on the
exit code alone. Exit 0 is not readiness.

Runner options for consumers: `--shell-bin` and `--desktop-bin` (default `.target/debug/flamewm-shell` / `flamewm-desktop`); both are spawned as owned children in the private session with the same environment plus `XDG_SESSION_TYPE=x11`, only inside the SS04/SS06/SS07 handlers, and stopped before the handler returns.
