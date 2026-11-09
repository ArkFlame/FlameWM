# Transparency recompose cost

Scope: baked backdrops on X11 (`set_surface_backdrop`, `desktop_backdrop`, window snapshot) for
settings, task manager, start menu, panels and widgets. Transparency stays on. The dock is out of
scope (see `docs/DOCK_BACKDROP.md`).

## Change (render-x11)

| Path | Before | After |
| --- | --- | --- |
| `app/surface_backdrop.rs` `set_surface_backdrop` | Always built RGBA, uploaded a new pixmap, freed the old one. | Identical size and RGBA keep the retained pixmap. Returns `changed`. |
| `surface_controller/backdrop_shape.rs` `set_surface_backdrop` | Always redrew. | Redraws only when the backdrop changed, or when clearing one that existed. |
| `app/window_backdrop.rs` canvas early return | Keyed on root and size only. A rebuilt canvas with the same root reused the old crop. | Also keyed on `canvas_epoch`. |
| `app/window_backdrop.rs` `capture_window_backdrop` | Uploaded every composed snapshot. | Skips the upload when root, size and pixels match the retained snapshot (generation unchanged). |

Shell callers are unchanged. `runtime/panels.rs` already skips an unchanged
`(rect, opacity, blur, source)` key before it calls `desktop_backdrop`.

## Measured

Unit level, `cargo test -p flamewm-render-x11 --lib`. This is a decision simulation, not an X runtime run.

| Scenario | Requests | Installs before | Installs after | Test |
| --- | --- | --- | --- | --- |
| One surface, pixels `A A A B B A` | 6 | 6 (no compare) | 3 | `app::surface_backdrop::tests::repeated_backdrop_requests_install_once_per_change` |

"Before" is the pre-change behaviour by construction. The old binary was not run against this sequence.

## Not measured

- Runtime recomposes per scripted scenario (settings open, task manager, start menu, panel
  redraw, widget repaint). No Xvfb scenario was run, so there are no before/after counts for these.
- Window snapshot skip: covered by `app::window_backdrop::tests::repeated_snapshot_is_unchanged_only_for_same_root_size_and_pixels`. No runtime count.

## Known costs left in place

- `backdrop/desktop.rs` `DesktopLayer::crop` decodes the wallpaper file on every call when the blur
  radius is below `2 * BLUR_DOWNSCALE`. The uncached crop is not changed here.
- `app/damage_plan.rs` `region_translucent` disables the region paint while a surface backdrop is
  installed. Relaxing that changes rendering and needs runtime evidence.

## Gate

- `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin scripts/gate flamewm-render-x11`
  (`GATE_TARGET=/run/media/linsaftw/SDD/target-transp`, `GATE_CLIPPY_TARGET=/run/media/linsaftw/SDD/target-transp-clippy`)
- Pass on the rebased commit: `EXIT_GUARD=0 EXIT_FMT=0 EXIT_CLIPPY=0 EXIT_TEST=0 GATE_DONE all=0`.
  Lib tests 454 passed, 0 failed.
- Flaky integration test: the first gate run had `EXIT_TEST=101` in `xdnd_two_windows_on_private_xvfb`
  ("target window appears"). Two reruns failed the same way, then a rerun skipped (no free Xvfb
  display). The origin/main binary passed once (1.25s). The test uses `backdrop: None`, so no changed
  path runs in it. It passed on the rebased commit in the second gate (0.98s, not a skip).
