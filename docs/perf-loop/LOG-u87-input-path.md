# u87-input-path

Area: pointer motion path (`crates/flamewm-wm-wayland/src/input/pointer.rs`): scene hit test,
hover/focus update, pointer constraints, and the per-event work a window drag repeats.

Composition estimate (share of the pointer-motion handler per event, from the call paths; no
absolute numbers by rule, and no build was run because this unit is forbidden `cargo`):

- before, hover/plain motion: one `front_to_back` walk plus its `Vec` allocation (~35%), one
  `with_pointer_constraint` lookup per question asked about lock and confine (~10%), smithay
  motion/constraint sync/frame (~25%), and, for a point no window covers (desktop, gaps), one
  `WindowModel::hit` window-stack scan per managed scene entry, i.e. quadratic (~30%).
- before, captured pointer (game / pointer-lock confine): the confined candidate and the final
  point were resolved independently, so every motion walked the scene twice. Hit-test share
  doubles to ~55-60%.
- before, window drag (move or resize grab): the full scene walk ran every event even though the
  dragged frame follows the pointer, and every changed rect rebuilt *every* toplevel's output home
  (`output_name_of` string alloc, `Output::enter/leave`, `SurfaceOutputs::assign`, plus
  `scale_snapshot`/`notify_scale_change`). With N toplevels that is ~2N string allocations and N
  surface clones per pointer event, and it dominated the drag path (~60-70% of the handler;
  hit-testing was under 15% there).

Changes in `input/pointer.rs`:

- `MotionConstraint` + `motion_constraint()`: lock and confine are one constraint record on the
  focused surface, so `on_motion` reads it once. `locked_focus()` now derives from that read
  instead of a second `with_pointer_constraint` call.
- `confined_motion()` replaces `confined_location()`: it returns the scene target already resolved
  at the accepted candidate, so `on_motion` reuses it for the hover/focus update instead of walking
  the scene again (`resolved` is dropped if the layout clamp moved the point). A confined pointer
  now walks the scene once per event instead of twice.
- `dragged_target()`: while a move or resize grab carries a window, the target is resolved from
  that window (rect origin, surface tree, `Hit::Resize` for a resize) instead of the full
  front-to-back walk and its allocation. The decoration band still yields no client focus, and the
  resize cursor still comes from the dragged edge, so the hover/focus and cursor results are the
  same for the dragged window.
- `home_output_changed()`: a drag rebuilds the toplevel output homes only when the new rect lands
  on a different output row (`output_for_rect`), i.e. exactly when `wl_surface.enter`, the
  fractional scale and the presentation-scale notification can change. In-output drag steps only
  queue the configure.
- `pointer_target()`: the managed hit is resolved lazily once for the whole walk
  (`Option<Option<Hit>>` filled on the first managed entry) instead of once per managed entry, so a
  pointer over the desktop or a gap costs one window-stack scan instead of N.
- Move grab: `titlebars` is read once and the floating frame is built once; `snap_target_at` (and
  with it the output name, work area and threshold lookups) now runs only when
  `snapPreviewEnabled` is on, because the zone ring is the only consumer of the target there.
  Release still snaps.

Expected composition shift: hover motion keeps one scene walk but loses the per-entry window-stack
rescan (empty-area points fall from quadratic to linear); confined motion halves its walk; a drag
drops the scene walk and the per-toplevel output-home rebuild entirely, leaving the configure
queue, the snap zone (only when enabled) and smithay dispatch. Visuals and features are unchanged:
same hover/focus target, same resize cursor on the dragged edge, snap ring and release snapping,
output `enter`/scale updates still fire on a cross-output step.

Verification: no `cargo` in this unit (shared checkout); changes were checked by reading the
affected call paths and the smithay 0.7.0 pointer grab (`ClickGrab` keeps the press-time focus, so
the drag-path target is only used for the pending focus and the cursor). Runtime composition
sample is UNVERIFIED until the orchestrator builds and samples.
