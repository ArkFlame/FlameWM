# u98-dead-code

Batch fourteen, dead-code cleanup across `crates/**/src`. `cargo` and `git` are forbidden in this
shared checkout, so the warning set was reconstructed by reading the tree: every private and
`pub(crate)` definition in the workspace was matched against its references outside test modules,
with the reference list split into DRM and non-DRM callers for `flamewm-wm-wayland`.

## What was actually dead in a release build

- `flamewm-wm-wayland/src/output_layout.rs`: `ModeCandidate` and `select_mode` are built and called
  only from `drm/outputs.rs`, which is compiled under `--features flamewm-wm-wayland/drm`. A plain
  `cargo build --release` (the workspace default, no feature) therefore saw the struct and the
  function as never constructed and never used, even though the DRM release binary needs both and
  the mode-policy tests pin both. Kept, and marked with the same guard the neighbouring
  `input/pointer.rs::relative_base` already uses:
  `#[cfg_attr(not(feature = "drm"), allow(dead_code))]`. The policy and its tests are untouched;
  only the non-DRM warning is silenced.
- `flamewm-desktop/src/widgets/render.rs`: `widget_surface` re-resolves a full settings snapshot
  and is now called only from `widgets/tests.rs`; the live sync path goes through the per-sync
  `surface_key` plus `widget_surface_from`. It is the reference the surface tests compare against,
  so it is kept under `#[cfg(test)]` instead of deleted. `WidgetSurface`, `surface_key` and
  `widget_surface_from` remain on the live path and are unchanged.

## Checked and deliberately kept

- Trait implementation methods (`impl WindowPort/WorkspacePort/SessionPort/… for X11Desktop`,
  `impl Drop`, `impl PartialEq`) are reachable through the trait and never warn.
- `event_pump.rs::has_pending`, `damage_plan.rs::plan_redraw`,
  `render-core/src/pixels.rs::blend_mask_span`, `integrations-linux` `child_pid` and
  `scene.rs::compose_front_to_back` are already behind `#[cfg(test)]`.
- `pub` library surface (`AssetImage`, `Progress`, `presented_on_vblank`, `refresh_feedback`,
  ports, control D-Bus types) is exported API, not dead code, and was not touched.
- `logind`/`startup` were already guarded with `#[cfg_attr(not(feature = "drm"), allow(dead_code))]`.

Expected composition shift: no work removed from any live path (these items never ran in the
default build). The change removes two rustc dead-code warnings and keeps the DRM mode policy and
the widget surface tests intact. Visuals and features are unchanged.

UNVERIFIED: no `cargo`/`git` was permitted for this unit, so this was verified by reading the
definition and every reference of each item, not by a compiler run.
