# Dock translucency over a bright window (X11): status

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

- Wayland: not checked. `docs/DOCK_BACKDROP.md` lists its own unverified case.
- Inherit with a light taskbar keeps its light alpha floor only when the dock is not custom (the
  dock override replaces it while custom).
- No performance claim. Standing decisions unchanged: transparency optimised, not disabled; no
  glass/shadow effects; no animations.
