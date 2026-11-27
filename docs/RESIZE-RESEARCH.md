# Resize research

Source-only survey of resize behavior in `.vendor/smithay-master` and
`.vendor/cosmic-epoch`, narrowed to what applies to
`crates/flamewm-wm-wayland/src/input/pointer.rs`.

## Smithay: xdg_toplevel resize state

- `.vendor/smithay-master/src/wayland/shell/xdg/mod.rs` — `ToplevelSurface::with_pending_state`
  (1698) lazily seeds `XdgToplevelSurfaceData::server_pending` from `current_server_state()`, then
  hands `&mut ToplevelState` to the caller. This is where a compositor writes a new size and the
  `ToplevelState::Resizing` bit for an interactive resize.
- Same file — `ToplevelSurface::send_configure` (1489) and `send_configure_internal` (1837) turn
  that pending state into a configure carrying a fresh `Serial`, appended to `pending_configures`;
  `send_pending_configure` sends only when the state actually changed.
- Same file — `XdgShellHandler::resize_request` (1114) is the compositor-side hook for a
  client-requested interactive resize (seat, serial, `ResizeEdge`).

## Smithay: configure serial ack

- `.vendor/smithay-master/src/wayland/shell/xdg/mod.rs` — the generated `ack_configure` (221) finds
  the pending configure whose serial matches, promotes it to `last_acked`, and retains only
  configures with a newer serial. `last_acked` is the field declared at 209-212.
- `.vendor/smithay-master/src/wayland/shell/xdg/handlers/surface.rs` — the `Request::AckConfigure`
  arm (267) calls that helper (302/310) and forwards the resolved configure to
  `XdgShellHandler::ack_configure` (334); an unknown serial raises `InvalidSurfaceState`.
- Narrow read: `last_acked.state.size` is the only authoritative "client caught up" signal. A
  compositor that sends the next resize configure before it changes is stacking requests.

## Cosmic-epoch: configure throttling until ack

- `.vendor/cosmic-epoch/cosmic-comp/src/shell/element/surface.rs` — `ShellSurface::latest_size_committed`
  (630) compares `current_server_state().size` against `cached_state.last_acked.state.size`; it is
  true only once the client committed the compositor's previous size. The comment names the reason:
  clients that do not batch size requests misbehave with very fast (1000 Hz) input devices.
- `.vendor/cosmic-epoch/cosmic-comp/src/shell/layout/floating/grabs/resize.rs` —
  `ResizeSurfaceGrab::update_location` (82) gates `self.window.configure()` on
  `latest_size_committed()` (177). The tiling twin, `src/shell/layout/tiling/grabs/resize.rs`
  (`ResizeForkGrab::update_location`, 330), uses the same gate at 442-453.
- `cosmic-comp/src/state.rs` — `throttle` (1344) is a separate *frame* throttle
  (`SCREENCOPY_THROTTLE` ~16.7 ms, `THROTTLE` 995 ms); it is not the ack gate.

## Cosmic-epoch: per-frame motion coalescing

- `cosmic-comp/src/shell/layout/floating/grabs/resize.rs` — `PointerGrab::motion` (202) forwards
  motion to Smithay, then `update_location` (82) stores the newest size in `last_window_size` (170)
  and configures only when acked: many motion samples collapse into one configure per ack/frame.
  `PointerGrab::frame` (258) delegates to `handle.frame`, the frame flush point.
- `cosmic-comp/src/shell/layout/tiling/grabs/resize.rs` — `ResizeForkGrab::update_location` (330)
  accumulates `accumulated_delta` across motion events, ignores rounded-zero steps (403), and
  pushes one `update_positions` tree update when `should_configure` holds (442-453).
- Narrow read: coalesce at a single pending-configure slot, not per motion event.

## Cosmic-epoch: last-buffer scaling

- `.vendor/cosmic-epoch/cosmic-comp/src/shell/layout/floating/mod.rs` — `FloatingLayout::render`
  (1552) reads `buffer_size = elem.geometry().size` (1603), builds a `Scale` from target over buffer
  geometry (1604-1607), and wraps the element in `RescaleRenderElement` (1612). The last committed
  buffer is therefore stretched to the new geometry until the client commits a fresh one, with no
  extra configure needed to keep the frame correct.
- There is no literal `last_buffer` identifier in this tree; the effect is render-side and lives
  outside pointer input, so it cannot be copied into `pointer.rs`.

## Cosmic-epoch: outline vs live

- `.vendor/cosmic-epoch/cosmic-comp/src/shell/mod.rs` — `Shell::set_resize_mode` (2528) and
  `Shell::resize_mode` (2555) own `ResizeMode` (Started/Active/Ended) and a `ResizeIndicator`;
  `cosmic-comp/src/backend/render/mod.rs` reads it at 767 and passes it to layouts at 1021 and 1060.
- `cosmic-comp/src/shell/layout/floating/mod.rs` — inside `FloatingLayout::render` (1660-1677) the
  indicator is resized to the focused geometry plus a 36 px margin and pushed as a render element,
  while the grab reconfigures the window only in live mode.
- Two modes result: outline draws the ring and commits once on release; live resizes the real
  geometry and configures per frame.

## Ranked applicability for pointer.rs

1. **Ack-gated configure throttle — highest.** `State::on_motion` (122) sets
   `pending_configure = Some(id)` on every changed live-resize rect (246) with no check of the last
   acked size. Adopt `ShellSurface::latest_size_committed` semantics before issuing the next one.
2. **Per-frame motion coalescing — high.** `grab_motion_needs_work` (643) already drops
   anchor-unchanged steps and the `pending_configure` slot already collapses writes; make the flush
   point explicit at the `pointer.frame(self)` call in `update_pointer_focus` (315) and gate it on
   ack rather than on raw motion count.
3. **Outline vs live — already present.** The `show_content_while_dragging` branch (235-255)
   gives live rect + configure versus ring-only + release commit; only naming could be aligned with
   `ResizeMode`. Low effort, low risk.
4. **Smithay configure/ack plumbing — supporting.** `with_pending_state`, `send_configure` and
   `ack_configure` supply the serial and `last_acked` that items 1-2 need; `pointer.rs` should
   consume that state, never duplicate it.
5. **Last-buffer scaling — lowest / out of scope.** It lives in `FloatingLayout::render` via
   `RescaleRenderElement`; `pointer.rs` has no render path, so no change belongs here.

PASS — every section above cites a concrete file path and function from the two vendors, and the
applicability list is ranked for `crates/flamewm-wm-wayland/src/input/pointer.rs` with line anchors.
Two brief labels map to differently named code rather than literal identifiers: "last-buffer
scaling" is the render-side `RescaleRenderElement`, and "outline vs live" is
`ResizeMode`/`ResizeIndicator` (mirrored locally by `show_content_while_dragging`). Items 1-2 are
the only ones that warrant a code change in `pointer.rs`; the survey is static because this task
forbids builds, tests and runtime checks.
