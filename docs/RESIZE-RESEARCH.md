# Resize research and Wayland improvement plan

## Scope and evidence

This compares the active FlameWM Wayland pointer resize path with the local
KWin, cosmic-epoch, and Smithay reference trees. The `.vendor` trees are
read-only research material, not runtime dependencies. Paths and symbols below
are the local source evidence; upstream revision identifiers were not recorded.

Active FlameWM owner: `crates/flamewm-wm-wayland/src/input/pointer.rs` handles
pointer motion during a resize; `crates/flamewm-wm-wayland/src/input/buttons.rs`
commits the final geometry on release. Outline resize is the default.

## Techniques that keep resize cheap

### KWin

- `src/x11window.cpp:interactiveMoveResize` and the nearby geometry update path
  resize from the current move/resize geometry and apply size constraints
  before committing with `moveResize`. `src/x11window.cpp:resizeWithChecks`
  contains the constraint operation. This keeps geometry policy centralized
  rather than repeating it in the pointer event path.
- `src/waylandwindow.cpp:resizeWithChecks` provides the corresponding
  constrained geometry operation for Wayland windows. Geometry is changed
  through window-level methods, not by having input code directly manipulate
  client buffers.

### cosmic-epoch

- `cosmic-comp/src/wayland/handlers/xdg_shell/mod.rs:resize_request` hands a
  protocol resize request into the shell's resize-grab machinery. The handler
  is the boundary between the xdg-shell request and compositor-owned grab
  state.
- `cosmic-comp/src/state.rs:send_frames` dispatches frame callbacks along
  rendered surface trees. The backend calls this in response to presented
  frames, so callback progress follows the compositor's frame lifecycle
  rather than every raw pointer event.
- `cosmic-comp/src/shell/element/surface.rs:send_frame` forwards callbacks
  through the surface abstraction. That lets frame delivery remain bounded by
  the compositor's presentation path.

### Smithay

- `src/wayland/shell/xdg/handlers/surface/toplevel.rs:request` dispatches
  `xdg_toplevel.resize` to `XdgShellHandler::resize_request`. The protocol
  handler delegates grab authorization and resize policy to the compositor.
- `src/wayland/shell/xdg/handlers/surface.rs:ack_configure` resolves the
  acknowledged configure serial and calls `XdgShellHandler::ack_configure`.
  Configure state is tied to its serial, allowing clients to acknowledge the
  configure they actually applied rather than treating a request as an
  immediate buffer resize.
- `src/desktop/wayland/window.rs:send_frame` and the surface-tree frame
  helpers deliver frame callbacks through the desktop window abstraction.
  Rendering/presentation therefore controls callback delivery, not pointer
  motion frequency.

## FlameWM gaps, ranked

1. **No explicit frame-paced configure policy for live resize.**
   `input/pointer.rs` updates the model on each changed pointer position and
   sets `pending_configure`; the paint/flush path can therefore send successive
   size configures at input-event frequency. Avoiding duplicate unchanged
   rectangles helps, but does not bound configure production to useful display
   frames. Add a frame-paced/latest-geometry policy at the resize-to-configure
   boundary, preserving the final release geometry. Likely owner:
   `crates/flamewm-wm-wayland/src/input/pointer.rs` plus the configure flush
   owner in `crates/flamewm-wm-wayland/src/input/handlers.rs`.
2. **No demonstrated serial-correlated resize completion behavior in the
   resize interaction path.** `buttons.rs` clears `pending_configure` after
   release applies geometry, while client buffer readiness is asynchronous.
   Audit and make the configure/ack/commit relationship explicit so a late
   configure or commit cannot regress the final target geometry. Likely owner:
   `crates/flamewm-wm-wayland/src/input/handlers.rs`; preserve the release
   commit in `crates/flamewm-wm-wayland/src/input/buttons.rs`.
3. **No demonstrated presentation-driven resize feedback policy.**
   `pointer.rs` requests a scene paint when resize state changes, but the
   resize path should be verified against frame callbacks and presentation
   cadence so clients are not encouraged to render more often than useful.
   Likely owner: `crates/flamewm-wm-wayland/src/input/handlers.rs` and the
   existing output/frame scheduling owner; do not move WM authority into UI.
4. **Outline mode trades responsiveness for low work.** The current default
   draws only a landing ring during motion and applies one final configure on
   release (`pointer.rs`, `buttons.rs`). Keep this safe fallback, while making
   any live mode use a bounded configure cadence rather than reverting to
   unbounded per-event work.

## Plan

Implement item 1 first: retain only the newest geometry during a resize and
flush it at most once per compositor frame, with a final release flush that
cannot be dropped. Keep configure serial tracking and client commit handling
in their existing protocol owner. Add focused tests for rapid motion, unchanged
geometry, release before a scheduled flush, and a late client commit. Measure
the resize path under a fixed pointer-event/frame cadence before claiming a
performance improvement. This research task does not implement the plan.

PASS — research and the ranked improvement plan are recorded from local
reference and active-source paths. No product code was changed; implementation
and performance claims remain unverified.
