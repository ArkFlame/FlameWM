# Dock translucency over a bright window (X11): status

> Rows: `dock-wl`, `dock-x11`, `dock-wire` in `docs/BACKLOG.md`.

Status: FIXED on X11 (pill alpha follows `dock.surfaceOpacity`; backdrop reinstated after hide).
Wayland is not covered.

## Root causes (verified)

| # | Cause | Fix | Evidence |
|---|---|---|---|
| R1 | Wire decoded `dock.surfaceOpacity` / `dock.surfaceBlurPercent` as `Integer`, so the shell's `percent_setting` fell back to 75 / 70. | Percent keys in `setting_value_for_key` (commit 32f28041). | `flamewm-control-wire` test `dock_surface_percent_keys_keep_percent_type_on_the_wire`. |
| R2 | The dock pill alpha ignored `dock.surfaceOpacity`. Inherit reads `glassOpacityPercent`, non-Inherit reads `taskbarOpacityPercent`; the dock override reaches neither. Defaults gave alpha 191 or 109. | `dock_pill_alpha` (shell `settings_projection/project.rs`): while `dock.surfaceCustom` is on and reduce-transparency is off, pill alpha = `dock.surfaceOpacity` (0 see-through, 100 opaque). | Test `dock_pill_alpha_follows_dock_surface_opacity_in_non_inherit_modes` (all three taskbar modes). |
| R3 | Hiding the dock cleared its surface backdrop (`window_state.rs` `hide`, kept on purpose: closed popups keep no stale pixels). The shell then got `Unchanged` from `LiveScene::compose`, whose composed-image key is shared across surfaces and was not reset, so the backdrop was never reinstalled on show. The pill then composited over the opaque scene clear `0x00232629` (depth 24), giving a dark pill. | `LiveScene::forget_composed` called when a surface backdrop is cleared (`hide`, `set_surface_backdrop(None)`) via `forget_live_composition`. | Test `forgotten_composition_is_sent_again` (`flamewm-render-x11` `app/live_backdrop.rs`). Runtime dumps before the fix: set 240x50, then a frame painted with no backdrop, presented `4d181b1d`. |

## Runtime evidence (after fix)

Private Xvfb `:97`, debug build from `/run/media/linsaftw/SDD/cargo-target-translucent`, white
xterm `-bg white -geometry 80x6+840+970` under the autohidden dock, dock settings from
`/tmp/claude-1000/verify2/settings.toml` with `dock.surfaceOpacity` set. Harness
`/tmp/claude-1000/transl/run2.sh OUTDIR OPACITY` (re-reveals the dock before each capture;
waits for two identical crops). Measure: mean luminance of the dock pill gap between icons,
crop x 113-119, y 10-39.

| Dock opacity | Expected (white beneath, alpha = opacity) | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 |
|---|---|---|---|---|---|---|
| 0 | 255 | 255.0 | 255.0 | 255.0 | 255.0 | 255.0 |
| 30 | 178 (alpha 77) | 178.0 | 178.0 | 178.0 | 178.0 | 178.0 |
| 70 | 76 (alpha 179) | 76.0 | 76.0 | 76.0 | 76.0 | 76.0 |
| 100 | 0 (opaque) | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |

Monotonic and identical across five runs per setting. Screenshot `fin-1-30/dock.png` shows the
white xterm through the pill.

## Limits

- Wayland dock pill: not isolated. The 2026-11-10 run below measured the bottom bar only.
  `docs/DOCK_BACKDROP.md` still lists the bright-window-beneath case as unverified.
- Inherit with a light taskbar keeps its light alpha floor only when the dock is not custom (the
  dock override replaces it while custom).
- No performance claim. Standing decisions unchanged: transparency optimised, not disabled; no
  glass/shadow effects; no animations.

## Wayland runtime (2026-11-10): bottom bar parity

Build: `cargo build --profile fast -p flamewm-wm -p flamewm-desktop -p flamewm-shell -p flamewm-wm-wayland`
at `a2cb7dae`. Private Xvfb 1920x1080, purple wallpaper `127,0,128` (fill), no window beneath.
Harness: `/tmp/claude-1000/dockwl/run2.sh` (copy of the parity harness; taskbar `surfaceCustom=true`,
`surfaceBlurPercent=70`, `surfaceOpacity` per row). Measure: band y 1036-1080, x 0-1600, mode colour.

| Backend | Opacity | Run 1 | Run 2 | Run 3 | Expected (black fill, alpha = opacity) |
|---|---|---|---|---|---|
| X11 | 70 | 38,0,38 | 38,0,38 | 38,0,38 | 38.1, 0, 38.4 |
| Wayland | 70 | 38,0,38 | 38,0,38 | 38,0,38 | 38.1, 0, 38.4 |
| X11 | 30 | 89,0,89 | | | 88.9, 0, 89.1 |
| Wayland | 30 | 89,0,89 | | | 88.9, 0, 89.1 |

Max per-channel diff X11 vs Wayland over the whole band (all runs at 70): 0 levels. Both backends
match the alpha-over-purple expectation, so the Wayland translucency path is runtime-correct for the
bottom bar, within the 2-level bar.

Not covered: the dock pill itself. With `dock.enabled=true` and `dock.surfaceCustom=true`, the band
stayed at the default taskbar fill (32,0,32 on both backends at dock opacity 30 and 70), so the
`dock.surfaceOpacity` override did not reach this band. The pill over a bright window beneath
(`docs/DOCK_BACKDROP.md:109`) stays unverified on Wayland.

## Wayland runtime (2026-11-10b): dock pill parity

Correction to the section above: its harness wrote `surfaceCustom` / `surfaceOpacity` without the `dock.`
prefix, so the Dock never took the override and the 32,0,32 band was the taskbar, not the pill. The
settings that paint the pill are `dock.enabled=true`, `dock.surfaceCustom=true`, `dock.surfaceOpacity`,
`dock.surfaceBlurPercent` (`crates/flamewm-api/src/settings_groups.rs` `dock_surface_custom`,
`DOCK_SURFACE_OVERRIDES`). The taskbar keeps the global surface and is unaffected by them.

Build: `cargo build --profile fast -p flamewm-wm -p flamewm-desktop -p flamewm-shell -p flamewm-wm-wayland`
at `4eed3b8b` (origin/main). Private Xvfb 1920x1080. Harness: `/tmp/claude-1000/dockpill/run3.sh`
(parity harness, dock keys above, `dock.surfaceOpacity=70`). Measure: pill interior strip x 941-946,
y 992-1012 (left of the trash icon, inside the pill), mean RGB, 3 runs each.

Backdrop: purple wallpaper `127,0,128` (fill). Bright: solid white wallpaper `255,255,255` (fill).

| Case | Backend | Run 1 | Run 2 | Run 3 | Expected (black fill, alpha 0.70) |
|---|---|---|---|---|---|
| purple | X11 | 38,0,38 | 38,0,38 | 38,0,38 | 38.1, 0, 38.4 |
| purple | Wayland | 38,0,38 | 38,0,38 | 38,0,38 | 38.1, 0, 38.4 |
| white | X11 | 76,76,76 | 76,76,76 | 76,76,76 | 76.5 |
| white | Wayland | 76,76,76 | 76,76,76 | 76,76,76 | 76.5 |

Max per-channel diff X11 vs Wayland: pill strip 0 levels (purple and white), taskbar band 1040-1076
x 0-1600 0 levels (purple and white). Pill spread within a run: 0.

Result: the dock pill is translucent on both backends and matches the alpha-over-backdrop expectation
over purple and over a bright backdrop. Within the 2-level bar.

Not covered (still open):
- Bright window beneath the pill, as a managed window. A bright wallpaper proves the blend, not the
  window path. The X11 live path covers managed Normal frames (F4), and Wayland windows are not in the X
  root (F11). Needs a managed window that sits under the pill; the WM clamps windows to the work area,
  and no Wayland client was used as a white window in this run.
- X11 unmanaged override-redirect white window: in one probe (`python-xlib` override window, 1920x1080, mapped
  before the WM) the pill still read purple-based 38,0,38 while the white window was visible at the same
  screen position outside the pill. Scope: unmanaged windows are not in `_FLAMEWM_REDIRECTED_FRAMES`, so
  this is a known limit of F4, not a regression. The Wayland probe window did not reach the screen, so
  there is no Wayland number for it.
- Opacity 30 and blur changes on the pill: not measured in this run.
