# LOG u95 — Wayland surface commit path

Area: `crates/flamewm-wm-wayland/src` (commit handler, popup bookkeeping, output lookup).

Ranked by composition before the change, a busy desktop's per-commit work broke down roughly as:
layer re-layout (whole layer map + every layer tree) ~half of the commits that come from panels,
then popup-tree scans, then the per-commit surface-name allocation, then the surface-tree walk.
Damage-dense clients (terminals, browsers) commit far more often than their geometry changes, so
the layer share was the largest avoidable slice.

## Changes

1. `handlers::commit` → layer re-layout is now gated on `resized || has_pending_changes()`.
   `LayerMap::arrange` walks every layer surface's surface tree (and each popup tree) and copies
   cached state per surface. It only depends on the layer's protocol state (anchor, size, margins,
   exclusive zone) and on the committed buffer geometry, never on damage. A panel animating or a
   notification repainting committed damage-only and paid a full re-layout each commit; that work
   is now skipped, and `reflow_if_work_area_changed` (which can only change through `arrange`) with
   it.
2. `handlers::commit` → popup `commit` + `find_popup` run only when `get_role(surface)` is
   `xdg_popup`. Both calls scan every known popup tree; toplevel/cursor/subsurface commits are
   never popups, so the common client path no longer scans popups.
3. `handlers::commit_changes_pixels` → early exit as soon as one surface in the tree shows a buffer,
   damage or frame callback. The callback only needs a boolean, so the rest of the subtree is not
   visited.
4. `handlers::commit` → one `buffer_geometry` read after `on_commit_buffer_handler` feeds both the
   resize reimport and the layer gate (was read twice).
5. `state::output_handle_of` → resolves the cached surface output name by reference instead of
   going through `output_name_of`, which allocated a `String` on every commit.

Visuals/features are unchanged: every arrange that the protocol state or a buffer resize requires
still runs, popups still map and get their first configure, and every commit that shows pixels
still requests a frame.

## Expected composition shift

Removes the layer-layout share from damage-only commits (largest single share for panel/notification
traffic), removes popup scans from all non-popup commits, and removes one `String` allocation plus a
partial tree walk per commit. Idle/steady-state repaint composition is unchanged; the win is
concentrated in clients that commit often without changing geometry.

## Left for a later unit

The DRM wake pipe is still pinged once per commit even when a paint is already pending. Coalescing
that needs a pending-ping flag cleared by the DRM loop; not worth the cross-file risk in this unit.
