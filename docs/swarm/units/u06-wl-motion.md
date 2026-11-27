# Unit u06-wl-motion

Status: **UNBUILT** — static source review and edits only. No cargo, git, build, test or runtime session was run in this unit; verification and commit belong to the orchestrator.

Area: Wayland pointer motion coalescing for interactive move/resize, including outline-only resize that commits on release.

Owned paths: `crates/flamewm-wm-wayland/src/input/pointer.rs`, `crates/flamewm-wm-wayland/src/input/buttons.rs`, `crates/flamewm-wm-wayland/src/input/move_feedback.rs`, this file. Workspace version: `0.1.7` (`Cargo.toml:6`, read directly because `scripts/version` needs python). Route: `flamewm-source-first`, `flamewm-rust-systems`, `flamewm-reference-mining`, `flamewm-parallel-execution` (loaded before edits). Git state inspection was not allowed by the brief.

## Review rows (current source)

| ID | Path / line | Finding | Action |
| --- | --- | --- | --- |
| B1 | `input/buttons.rs` move release, old line 85 | `snap_preview.hide()` dropped the ring buffers, but `model.commit_move` / `apply_geometry` do not set `scene_dirty`; only the resize release called `request_paint()`. A visible ring could stay on screen until an unrelated paint, and an outline-mode move jumped to its final rect with no paint of its own. | Fixed: capture the hide result, then `release_needs_paint(ring_hidden, moved)` requests the paint after the commit. |
| B2 | `input/buttons.rs` resize release, old lines 133-138 | Outline mode sends the final configure with `apply_geometry`, while `pending_configure` may still hold the same window from a live-mode step; the paint-side `flush_pending_configure` (`state/mod.rs:710`) then sends a duplicate configure. | Fixed: clear a matching `pending_configure` immediately after the release configure. Live mode is untouched and still defers its configure to paint. |
| B3 | `input/pointer.rs` move/resize branches; `input/buttons.rs` releases | The same rounded pointer-delta expression was duplicated in four places. High-rate sub-pixel samples that round to the same integer step produce identical geometry, but the contract was implicit. | Added pure `grab_delta` in `move_feedback.rs` and used it on all four paths. No rounding behaviour changed. |
| B4 | `xwayland.rs:866`, `handlers.rs:412` | Both client-initiated resize paths install `Grab::Resize` without a compositor button press, so outline geometry is committed only when a later hardware `ButtonRelease` reaches `on_button`. That generic release path is correct when the backend delivers the release; if no release ever arrives the ring stays visible and the resize never commits. | Reviewed only. Both paths are outside the owned files, so no edit was made; recorded as a risk for the orchestrator. |
| B5 | `handlers.rs:511`, `xwayland.rs:706` | Grab cancellation on window destroy clears `self.grab` but does not hide `snap_preview`; a ring can outlive its window. | Reviewed only, outside the owned files. Recorded as a risk. |
| B6 | `input/pointer.rs:177-221`, `input/move_feedback.rs` | `snap_preview_enabled = false` suppresses only the snap-zone ring; `move_feedback(false, None, ..)` still outlines the free landing frame in outline mode. `show`/`hide` return real change bits and the step already skips configure/paint when the rounded geometry or ring is unchanged. | No bug found; behaviour preserved. |
| B7 | `input/pointer.rs` resize branch, live mode | A runtime `showContentWhileDragging` toggle mid-resize-grab leaves the outline ring from the previous mode visible: the live branch sets the rect but never touched `snap_preview`, and no later release could arrive before another paint. | Fixed: the live branch hides the ring and repaints only when it was actually visible. |
| B8 | `input/pointer.rs:151` | Even with `grab_delta`, every input sample repeated the snap-target/output/work-area lookups, model writes and ring `show` call. Geometry only depends on rounded values, so sub-pixel samples inside one pixel produce identical results. | Fixed with a pure step key (rounded delta + rounded absolute point): the grab geometry block runs only when a sample crosses one of those boundaries. Pointer position, focus/hit routing and the release commit still run on every event, so the final geometry is unchanged. |

## Reference rows (`.vendor/**`, read-only evidence)

| Vendor / path | Semantic finding | Active decision |
| --- | --- | --- |
| `.vendor/smithay-master/src/input/pointer/mod.rs:224-307`, `anvil/src/input_handler.rs:1033-1096` | `motion` and `relative_motion` queue focus and dispatch immediately; `frame` only terminates a protocol frame. Anvil sends one motion (and relative motion) plus one frame per input event. Smithay groups client-visible events, not compositor geometry. | Our `update_pointer_focus` already pairs `motion` with `frame` on every event; keep that client-visible cadence. |
| `.vendor/cosmic-epoch/cosmic-comp/src/input/mod.rs:452-507`, `backend/render/cursor.rs:903-945` | Relative motion is sent per event; a grabbed resize can early-frame on a cross-output step. The one "Coalesce bursts from high-frequency pointers" site throttles the cursor idle-hide timer, not move/resize geometry. | No geometry precedent to copy. |
| `.vendor/mangowm/src/layout/arrange.c:1008-1020` | Closest precedent: `resize_tile_client` returns when `last_resize_frame == frame_serial`, so a burst of key repeats or pointer motion recomputes tile geometry once per output frame; `input/pointer.c:1186` still sends relative motion per event. | Adopt the *rule*, not the code: latest pointer sample wins, one geometry apply per output frame. |

## Coalescing decision row

| What | Decision |
| --- | --- |
| Full per-output-frame coalescing (store latest position, apply once at a frame callback; batch relative deltas) | **Not implemented.** A frame latch needs a pending-motion field on `State` and a paint-side flush beside `flush_pending_configure` (`state/mod.rs`) / the render passes; those files are outside this unit's ownership. Deferring `pointer.frame()` or hiding a thread-local sample would change client-visible event cadence or state ownership, i.e. behaviour change. Logged for the orchestrator as an owned-path follow-up. |
| Safe subset: coalesce grab geometry per integer pixel step (B8) | **Implemented.** `grab_position_key` captures the four rounded values the geometry can observe (offset from anchor and absolute point); `grab_motion_needs_work` skips the whole geometry block when a sample crosses none of them. Position, constraints, focus/hit routing, cursor and the release commit still run on every event, so a burst collapses to its last sample without changing the committed rect, the ring or client-visible motion frames. Combined with `grab_delta` and the existing `unchanged` / `show` / `hide` guards, repeated steps cost no configure, ring raster or paint work. |

## Unit tests added

`move_feedback.rs`:

| Test | Contract |
| --- | --- |
| `grab_delta_uses_the_latest_rounded_sample` | Positive/negative rounding (halves away from zero), sub-pixel collapse, zero step. |
| `release_repaints_for_hidden_ring_or_committed_geometry` | Truth table for the release paint rule. |

`pointer.rs` (`pointer_constraint_tests`):

| Test | Contract |
| --- | --- |
| `grab_key_uses_rounded_offset_and_absolute_point` | The step key is the rounded anchor offset plus the rounded absolute point. |
| `subpixel_grab_motion_is_coalesced` | Samples inside one pixel and an identical sample need no geometry work. |
| `grab_motion_crossing_any_rounded_boundary_is_processed` | A new offset step or a new absolute point step forces recomputation. |

Existing `move_feedback`, `needs_scene_rebuild`, two-mode and pointer-constraint tests were left unchanged.

## Verification

Not run, by task rule (no cargo, no git, no builds, no tests, no runtime). Edits are **UNBUILT**: the orchestrator must compile and run the `flamewm-wm-wayland` unit tests and check the release paths above on a live Wayland session. Static review only; no runtime or performance claim is made here.
