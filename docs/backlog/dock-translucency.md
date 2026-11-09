# Dock translucency over a bright window (X11): findings

Base: `origin/main` at `f528ab83`. Status: OPEN. The visible bug is not fixed. One verified
root-cause fragment is fixed (wire typing of `dock.surface*` percent keys). Runtime evidence:
private Xvfb `:97`, fast debug build from `/run/media/linsaftw/SDD/cargo-target-translucent`,
white xterm (`-bg white`) under the autohidden dock, dock revealed by pointer sweep to y=1079.
Harness: `/tmp/claude-1000/transl/run.sh OUTDIR OPACITY` (screenshots `OUTDIR/dock.png`).

## Verified

| # | Fact | Evidence |
|---|---|---|
| V1 | Wire decode types `dock.surfaceOpacity` / `dock.surfaceBlurPercent` as `Integer`, not `Percent`. The shell's `percent_setting` accepts only `Percent`, so the Dock silently uses the default 75 / 70. | `crates/flamewm-control-wire/src/encode.rs` `setting_value_for_key` (percent list had no `dock.*` keys). Shell dump before fix: `raw=Some((Some(Integer(30)), ...))`, `opacity=75`. |
| V2 | After the wire fix the shell receives `Percent(30)` / `Percent(70)` and the backdrop percent is the configured opacity. | Shell dump after fix: `raw=Some((Some(Percent(30)), ...))`, `opacity=30`. |
| V3 | The live backdrop is composed correctly under the dock: the snapshot words read `0xffffffff` at the dock centre when the white xterm lies beneath. | `DUMPDOCK ... px_center=ffffffff` in every run. |
| V4 | The live path ignores opacity. `LiveScene::compose` takes only `blur_percent`; words are opaque (`0xff` alpha). Opacity is applied only by the pill fill. This is by design. | `crates/flamewm-render-x11/src/app/live_backdrop.rs` `compose`; `backdrop/blur.rs` `blurred_argb_words` returns `opaque_argb_words`. |
| V5 | The Dock pill fill is `taskbar_fill` = `dock_fill` = glass material (`glass::resolve(Dock, ..)`). In non-Inherit modes it does not read `surfaceOpacity`, so the pill alpha stays at the material alpha (191 with defaults) whatever `dock.surfaceOpacity` says. | `crates/flamewm-shell/src/settings_projection/project.rs` (Dock branch, `taskbar_fill`); `crates/flamewm-ui-core/src/surface.rs` `dock_fill`. |
| V6 | Over a white window with material alpha 191, a black fill gives `255 * (1 - 191/255) = 64`. This is the reported flat 64. | Arithmetic from V5 plus the default alpha. |

## Not explained (open)

The pill renders dark regardless of `dock.surfaceOpacity`. Measured interior (`x 880-890, y 982-986`, mean luminance, white xterm beneath, `FLAMEWM_DUMP_DOCK` on):

| Dock opacity | Pill alpha expected (V5 fix) | Measured interior |
|---|---|---|
| 0 | 0 (white expected) | 20 |
| 30 | 77 (178 expected) | 21 |
| 50 | 128 | 18 |
| 70 | 179 (77 expected) | 11 |
| default material (75) | 191 (64 expected) | 9.7 (earlier run), 64 (other run) |

Runs are not stable: the same settings gave 9.7 and 64 at default material. The dump shows the backdrop as white at the dock centre in every run, so the darkening happens after the backdrop blit. Paint trace (`FLAMEWM_DEBUG_BACKDROP=1`) shows the final dock paint blits a `240x50` backdrop, then the pill is filled. Stages still unverified:

- S1 Pill fill colour/alpha at XRender time (`FillRect` path, `fill_rounded_rect` in `crates/flamewm-render-x11/src/xrender_aa.rs`). Not dumped.
- S2 `NodeBackdrop` commands for the dock (`crates/flamewm-render-x11/src/app/surface_backdrop.rs` `paint_node_backdrop`). Not checked whether dock nodes emit them.
- S3 Region paint and present (`crates/flamewm-render-x11/src/app/render/paint_region.rs`, `present_scene`). Not checked whether the blitted backdrop survives to the window.
- S4 Trace shows `size-mismatch backdrop=202x50 scene=240x50` before the 240-wide snapshot is applied. Earlier paints with a mismatched or missing backdrop use the transparent clear, which a 24-bit X window shows as black.

## Next step

1. Dump the pill colour and alpha at the `FillRect` for `dock-bar` (`paint.rs`) and the scene pixel after `paint_surface_backdrop`, same run, to split S1 from S3.
2. Log `NodeBackdrop` emission for the dock node (S2).
3. Re-run with a fixed sweep-and-hold harness, at least three runs per setting, before quoting numbers.

## Fix in flight (product code)

- Wire typing (V1): add `dock.surfaceOpacity`, `dock.surfaceBlurPercent`, `dock.surfaceOutlineOpacity` to the percent list in `setting_value_for_key`. Regression test `dock_surface_percent_keys_keep_percent_type_on_the_wire` in `crates/flamewm-control-wire/src/tests.rs`.
- Pill alpha from `dock.surfaceOpacity` (V5): tried and reverted. It did not change the measured interior, so it is not the fix. It is not in the tree.

## Limits

- Wayland is out of scope here. `docs/DOCK_BACKDROP.md` lists its own unverified case.
- No release or perf claim. Standing decisions unchanged: transparency optimised not disabled, no glass/shadow effects, no animations.
