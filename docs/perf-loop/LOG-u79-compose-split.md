# u79-compose-split log

Batch: within-thread split of the nested compositor's idle second. Owned paths:
`crates/flamewm-wm-wayland/**`, `crates/flamewm-profiler/**`. `git`/`cargo` are forbidden in the
shared checkout, so every check here is a source read; the orchestrator builds.

## Starting state

The compose scaffolding had already landed in the batch: `profile.rs` holds the static span labels
(`wmwayland.dispatch.{wayland,flush,control,input,resize,render,xwayland}`, `wmwayland.timer`) and
`install_compose_dump` prints the profiler window once per second when `FLAMEWM_PROFILE_COMPOSE=1`.
Spans were wired in `main.rs` (dispatch, flush, control, input, resize), `render/mod.rs`,
`xwayland.rs`, and the frame-pacing timer.

Three gaps kept that dump from answering the unit's question:

1. the window listed spans by milliseconds, but never by **share of the sampled second**, so "which
   subsystem owns the second" had to be divided by hand;
2. `after_dispatch` started its own `wmwayland.dispatch.control` span while `main` already wraps the
   call in the same label, so control was counted twice per turn and looked like the largest
   subsystem regardless of what it did;
3. only the winit path had a `render` span; the DRM `render_all` was invisible, so the same label
   read zero on the native backend.

## Changes

1. **`COMPOSITION` share block** (`flamewm-profiler/src/report.rs`). After the existing rankings,
   each span's `self_wall` is printed as a percentage of the measured process window
   (`proc_wall_ns`, the real monotonic delta), plus a clamped `wait` residual for unspanned time
   (the loop parked in `poll`). Self wall is used so a wrapper span cannot claim its child's time
   twice; the residual is clamped because another thread's spans can sum past the window. Expected
   shift in the instrumented idle second: `wait` should be the dominant share, and any real
   subsystem share is now directly comparable instead of re-derived.

2. **One control span per turn** (`control/service.rs`). `after_dispatch` no longer starts its own
   `profile::control()`; the caller owns that span. Removes one static-slot lookup per loop turn
   and makes the control share truthful. `turn` keeps its span (it is called from the bus source,
   not from the post-dispatch hook).

3. **Render span on the DRM paint path** (`drm/events.rs`). `render_all` now starts
   `profile::render()`, so the composition split works identically on the tty backend.

4. **Skip byte-identical cursor-only frames in the nested backend** (`render/mod.rs`). With a
   host-drawn pointer (named/hidden cursor), `request_cursor_paint` only sets the redraw flag, never
   the scene flag. `render_frame` used to rebuild the whole element list, re-check both backdrop
   captures and submit an identical frame for every pointer-motion request. It now consumes
   `take_scene_dirty()` and returns when the scene is clean, nothing is being dumped, the session is
   not locked, and the host draws the pointer. Everything that changes pixels sets the scene flag
   again (`request_paint`/`request_redraw`), and client commits with a buffer, damage or frame
   callbacks already call `request_redraw`. Expected composition shift: pointer motion in the
   nested session drops the `render` share to zero work per motion event; the share moves to
   `input`/`dispatch`, and the idle second's `wait` share grows.

5. **No per-frame intermediate element `Vec`** (`render/mod.rs`). `build_elements` now appends the
   scene straight into its own buffer through a new `scene_elements_into`; the owned
   `scene_elements` wrapper is kept for the Dock-backdrop capture iterator. Removes one heap
   allocation plus a full element copy per painted frame in both backends.

## How to read the split

Set `FLAMEWM_PROFILE_COMPOSE=1` and read the once-per-second
`FLAMEWM-WAYLAND-COMPOSE tick=N window=1000ms` block; the `COMPOSITION:` rows are the per-subsystem
shares and `wait` is the parked remainder. `FLAMEWM_PROFILE=1` alone keeps the normal long-interval
report, which now carries the same block. `scripts/perf-compose PID` stays the cross-thread
(per-thread-name) view; this block is the within-thread view it was missing.

Runtime evidence: not executed in this unit (shared checkout: no `cargo`, no running compositor
restart). Changes are source-verified only; the orchestrator's build and nested run provide the
numbers.
