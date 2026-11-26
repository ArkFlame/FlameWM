# u86-render-skip changes

Area: Wayland render path (`crates/flamewm-wm-wayland/src/render`). The three asks were skip
work for undamaged frames, skip occluded/minimised windows, and avoid blurring an unchanged
backdrop. The last one is already in place, so this unit implements the missing second one and
records why the first third need no further change.

- render/mod `scene_elements_into`: a managed window is no longer added to the element list when
  it cannot contribute a pixel. Two conditions cull it before any surface, outline, titlebar or
  backdrop element is built:
  1. **Off-screen**: its physical `frame()` does not overlap the physical output rect. This is
     the multi-output case (windows on the other layout outputs are not "hidden" in the model)
     and a window dragged past the edge; the renderer previously built every element and then
     clipped it away.
  2. **Fully occluded**: its physical frame fits inside one opaque region already produced by a
     window in front of it (the traversal is front to back). A decorated window's opaque
     `client_backdrop` body covers its full client rect, so an opaque window in front of a
     smaller one hides it completely.
  Opaque regions come from `Element::opaque_regions` of the window's own solid/surface/rounded
  elements, shifted by the element geometry (the trait reports them element-local, exactly as
  `smithay`'s own batching code re-offsets them). Memory elements (titlebar, outline strips) are
  never trusted as occluders: they carry rounded/translucent chrome and a full-rect claim would
  hide a window behind glass. Occluders are capped at 64 per frame; past the cap the lower
  windows are simply built as before, so the cost stays bounded and correctness does not depend
  on any cache.
  Soundness: `OutputDamageTracker` already damages the previous geometry of elements that vanish
  from the list (and subtracts the current opaque regions), so a culled window does not leave
  stale pixels; the occluder repaints that area.
  Composition shift: each frame with stacked windows builds N−(covered) element lists instead of
  N. Element construction is per-window work (surface tree walk, clip program lookup, decoration
  cache lookup/program, 4 strip uploads for the outline, backdrop crop) and it sits in the
  compose slice on every scene-changing frame; a maximised window over smaller ones makes the
  whole hidden-window share disappear. Rebuilding the list also allocates one `Elem` per surface
  element, so the culled share drops allocations too.

## Already satisfied (no code change)

- **Re-blur of an unchanged backdrop**: `refresh_desktop_backdrop` compares output size plus the
  desktop layer's `CommitCounter`s (`Decorations::desktop_current`) and skips the readback/blur;
  `refresh_dock_backdrops` keys its capture on the commits of the elements beneath the Dock
  (`BackdropKey::scene`); `Decorations::surface_backdrop` keeps one buffer per rect and
  `set_desktop`/`drop_desktop` clear it when the capture they were cropped from changes. Flat
  (`reduceTransparency`, the default) drops every capture instead of blurring.
- **No-surface-damaged frames**: the flat scene no longer produces `WinitEvent::Redraw` (paint
  requests are explicit); `render_frame` already consumes the scene flag and returns early for a
  host-drawn pointer, and the DRM path reuses the cached scene for cursor-only paints
  (`SceneBuild::CursorOnly`) and skips `Draw` for `Idle`/`Deferred` outputs. `DamageTracker`
  separately returns `skipped` when nothing is damaged.

## Checks

Static reading, `rustfmt --edition 2024 --check` clean. No cargo, no git in the shared checkout:
compile and tests are the orchestrator's step. Added `occluder_tests` for the containment rule
(equal rect covers, larger covers, one-pixel overhang does not, disjoint does not, empty never
culled).

## Skipped

Occlusion by layer surfaces, unmanaged windows, popups and the blue-light overlay: those owners
could also be occluders, but their opacity is chrome/translucency dependent, and leaving them out
keeps the rule conservative. Windows without server-side decoration are only culled when their
client surface declares opaque regions, which is the honest answer for alpha buffers.
