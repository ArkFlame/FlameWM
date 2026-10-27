# Verification

## Mandatory target-machine gate

```bash
./scripts/doctor
./scripts/check
./scripts/xephyr
```

`./scripts/check` executes:

1. static architecture/dependency audit;
2. FlameWM Render 0.0.6 source promotion audit;
3. Cargo metadata;
4. workspace/all-target compiler check;
5. workspace tests;
6. complete release workspace build;
7. V8 HTML/CSS -> RWRB/2 release compile;
8. RWRB/2 inspect smoke gate;
9. active-package font-binary guard.

Formatting/Clippy is intentionally a separate hygiene gate:

```bash
./scripts/lint
```

It must not obscure the compiler/runtime gate when diagnosing a release candidate.

## Visual gate

Default product mode launches `scripts/xephyr` with the hard T06/T07 and SR canary lane and captures a fresh root screenshot. It validates that root image only; it does not verify Settings, popups, or other interactive states.

```bash
./scripts/verify-visual
```

Reference mode compares two existing images and does not launch a session. Supply both paths explicitly:

```bash
FLAMEWM_VISUAL_CURRENT=/path/to/current.png \
FLAMEWM_VISUAL_REFERENCE=/path/to/reference.png \
./scripts/verify-visual --reference
```

Reference mode requires matching image geometry and reports RMSE and absolute-error metrics without resizing. Those metrics are advisory unless both `FLAMEWM_VISUAL_RMSE_MAX` and `FLAMEWM_VISUAL_AE_MAX` are set. The tool does not establish oracle approval.

Historical reference (not present in this checkout; not current oracle approval):

```text
project_sources/visual/rwr-0.0.6-target-xephyr.png
```

## Wayland session gate

The Wayland session has its own gates; none of them replaces a hardware login. Source, wrapper and
packaging evidence:

```bash
./scripts/tests/wayland-session-matrix.sh          # wrapper backend selection and capability gate matrix
cargo test -p flamewm-wm-wayland --features drm    # logind state machine, frame planner, probe tables
./scripts/build-deb                                # preflight + drm_compiled=true gate + packaged session entries
```

Only behaviour that was actually executed may be reported. `--capabilities` is compile-time only
(`flamewm-capabilities=1`, `drm_compiled=true|false`, `backend=native|nested`); `--probe` is advisory
and never gates startup.

### Required hardware procedure (not yet performed)

1. Install the `.deb` from a working X11 session so recovery stays available.
2. `flamewm-wayland --capabilities` must show `drm_compiled=true`.
3. Log out and select `FlameWM (Wayland)` in the display manager (LightDM).
4. On failure collect `~/.cache/flamewm/wayland-session.log` (with its `.1` rotation) and
   `journalctl -b`; the `FLAMEWM-WAYLAND-STARTUP stage=... status=... code=...` lines name the
   failing stage.
5. On success verify: first frame on the connector, panel and shell visible, a terminal opens and
   accepts input, pointer and keyboard work, Ctrl+Alt+Fn VT switch away and back, and a further
   login after returning to the VT.
6. Recover by choosing `FlameWM` (X11) at the login screen; the Wayland wrapper does not fall back
   by itself.

No successful native DRM/LightDM login has been recorded. A login that returns to the greeter after
showing a text cursor has been reported and is under investigation, so a report must not claim a
hardware login it did not perform. See [`WAYLAND_PLAN.md`](WAYLAND_PLAN.md).

## Release gate status

Run these gates on the current checkout and report their actual results. A source audit or package build alone does not establish compiler or runtime verification. `scripts/release-gate` reports its Xephyr runtime and visual gates as `UNEXECUTED` until they are run separately.
