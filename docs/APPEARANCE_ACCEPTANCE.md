# Appearance native acceptance (AP01..AP09)

Runner: `tools/flamewm_appearance_acceptance.py`, wrapper: `scripts/appearance-acceptance`.
Unit tests: `python3 -I tools/test_flamewm_appearance_acceptance.py` (pure pixel, monotonic,
parsing and schema functions; no display is touched).

## What is measured

| ID | Observation |
|----|-------------|
| AP01 | Taskbar strip pixels (bottom 44 px) match `taskbarColor` at `taskbarOpacity=100` (>=50% of pixels). |
| AP02 | Start popup (`FlameWM start` window) interior matches `startMenuColor`; taskbar strip unchanged. |
| AP03 | Accent pixels on the taskbar equal `accent` at `accentOpacity=100`; at 40 they equal the alpha blend. |
| AP04 | Frame top-left corner is cut in Rounded and not cut when maximized or snapped left; iOS top row differs from the title row. Fullscreen is not driven, so AP04 is never PASS. |
| AP05 | Titlebar pixel equals `titlebarColor`; close hover equals `#e81123` by default and the custom `titlebarCloseHoverColor`; minimize button unchanged by close hover. |
| AP06 | Start popup `y` sampled during opening is non-increasing with >=3 distinct steps (slide-up); with `animationsEnabled=false` it is constant. |
| AP07 | A transient window appears during minimize and narrows; the client is unmapped; `_NET_WM_ICON_GEOMETRY` lies in the taskbar strip. |
| AP08 | Taskbar region changes between `iconTheme` values; cursor ARGB (XFixesGetCursorImage) hash differs between cursor themes. |
| AP09 | WM and Shell CPU time over 5 s <= 0.05 s and voluntary context switches <= 10 (from `/proc`). |

Every AP writes its screenshots under `DIR/shots/`. Status vocabulary is
`PASS`, `FAIL`, `NOT_RUN`, `ENVIRONMENT_BLOCKED`. A PASS needs a measured observation.

## Requirements

- Host `DISPLAY` set, `Xephyr`, `xwininfo`, `xprop`, `xdpyinfo`, `import`, `identify`, `dbus-run-session`.
- Python Pillow (`PIL`) for pixel reads. XFixes via `libXfixes` (ctypes) for AP08 cursor.
- Release binaries: `.target/release/flamewm`, `flamewm-shell`, `flamewm-settings` (or `--*-bin`).
- `xclock` as the decorated client (`--client-bin`).

## Usage

```sh
scripts/appearance-acceptance --run-dir "$SCRATCH/appearance-$(date -u +%Y%m%dT%H%M%SZ)" \
  --geometry 1280x800 --display :97 --settle-seconds 3
```

- `--run-dir` must not exist, or must be empty. It must not live under `.debug`, `.performance`,
  `.target` or `.agents/runtime`.
- `--display` must be in `:90`..`:99`; an in-use display is refused.
- The runner re-execs itself under `dbus-run-session` with `FLAMEWM_AP_INNER=1`.
- Settings are seeded as `revision=2` plus `key=value` lines in
  `DIR/xdg-config/flamewm/settings.toml` before each WM+Shell restart.
- Optional: `--icon-theme-alt NAME` (default `breeze-dark`), `--cursor-theme-base NAME`
  (default `McMojave-cursors`), `--cursor-theme-alt NAME` (default `FlameWM-Breeze-Dark`),
  `--taskbar-slot X,Y,W,H`. The slot is not yet compared to AP07 geometry (see Limits).

Exit: `0` no FAIL, `1` any FAIL, `2` usage error, `3` environment blocked. `0` is not
readiness; read `verdict` in `result.json`.

## Limits

- Taskbar placement assumes a bottom panel of 44 px.
- AP04 fullscreen and per-button minimize/maximize hover colors beyond close are not driven.
- AP07 checks that `_NET_WM_ICON_GEOMETRY` lies in the taskbar strip; equality with a
  shell slot rect is not measured.
- AP07 uses the root-visual fallback when no 32-bit visual is reported (`xdpyinfo`); the
  capability is recorded in evidence.
- No live run has been executed by the author of this runner; treat results as unverified
  until a fresh run on a private Xephyr produces `result.json`.
