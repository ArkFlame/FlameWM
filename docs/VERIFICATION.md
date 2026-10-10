# Verification — 0.0.9

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

Reference:

```text
project_sources/visual/rwr-0.0.6-target-xephyr.png
```

## Release gate status

Run these gates on the current checkout and report their actual results. A source audit or package build alone does not establish compiler or runtime verification. `scripts/release-gate` reports its Xephyr runtime and visual gates as `UNEXECUTED` until they are run separately.
