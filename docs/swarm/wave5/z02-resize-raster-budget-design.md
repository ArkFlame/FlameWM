# z02 — Resize outline raster budget

Design doc for the `flamewm-wm-x11` resize outline. Implementation in this slice is **step 1 only**
(pure gate + tests, UNBUILT). The full plan is recorded here so later steps land as independent,
compilable commits.

## Current structure (file:line)

- `crates/flamewm-wm-x11/src/frame/controller/planning.rs:251-311` — `resize_step`. At `:289-296`
  every live motion with `!contents_while_dragging && !final_effects` emits
  `FrameEffect::DragOutline { rect: Some(plan.frame) }`.
- `crates/flamewm-wm-x11/src/frame/controller.rs:86-89` — `DragOutline` effect contract.
- `crates/flamewm-wm-x11/src/wm/effects.rs:226-246` — `execute_effects` applies `DragOutline`
  by calling `self.snap_preview.show_outline(...)` on every effect, with no pacing.
- `crates/flamewm-wm-x11/src/snap_preview.rs:217-224` — `show_outline`; `present` at `:226-274`
  issues `configure_window` (`:246`), `change_window_attributes` (`:248-254`), `clear_area`
  (`:256`) and `shape_rectangles` (`:260-272`). The only dedup is an identical-geometry check
  at `:219`, which never fires on a moving pointer.
- `crates/flamewm-wm-x11/src/live_settings.rs:285-288` — `show_content_while_dragging()` defaults
  to `false`, so the uncapped outline path is the default resize mode.
- `crates/flamewm-wm-x11/src/wm/effects.rs:3-17,55-98` and
  `crates/flamewm-wm-x11/src/wm/mod.rs:560-562` — a 16 ms pace already exists for live-resize
  configure batches, but it is gated on `show_content_while_dragging()` (`:61`) and therefore
  never covers outline mode.
- In-repo precedents for the typed gate shape:
  `crates/flamewm-shell/src/app/progress_gate.rs:10-35` (`PaintDecision::{Now, Wait}`) and
  `crates/flamewm-wm-x11/src/wm/paint.rs:24-45` (`chrome_paint_due` / `chrome_wait_cap`).

## Counted problems

1. **Unbounded outline rasters** — `effects.rs:231-244` + `snap_preview.rs:218-272`: one motion
   produces one full configure/shape pass. A 1000 Hz motion stream produces 1000 X request
   batches per second; this is the resize freeze.
2. **Default mode is the unsafe one** — `live_settings.rs:285-288`: the capped configure path is
   skipped entirely in the default outline mode.
3. **No trailing edge** — `snap_preview.rs:219` only suppresses exact repeats; a paced implementation
   must still present the newest geometry once the interval elapses, or the outline would end
   behind the pointer.
4. **No reactor deadline** — `wm/mod.rs:560-562`: configure pacing has state, outline pacing has
   none, so no tick can be woken for a queued outline raster.
5. **Asymmetric counter coverage** — `effects.rs:76` counts throttled configure batches only;
   outline throttling is invisible to telemetry.

## Target design

A pure, typed gate in `crates/flamewm-wm-x11/src/wm/resize_raster.rs`:

- `OUTLINE_RASTER_CAP_PER_SEC = 60`; spacing is `ceil(1000ms / 60) = 17ms`, so a 1 kHz stream
  admits at most 60 rasters in any one-second window **including** the trailing flush.
- `OutlineRasterGate<T>` (typed facade; `T` is the geometry payload, live caller supplies
  `RootRect`). `note(now, geometry) -> RasterDecision::{Now, Wait(deadline)}`. While paced, the
  newest geometry replaces the stored one (trailing edge, latest wins).
- `take_due(now) -> Option<T>` is a one-shot trailing flush that consumes the next slot;
  `cap_wait(now, wait)` caps the reactor wait to the deadline; `clear()` drops pending state on
  ungrab/cancel.
- No X calls, no new deps, no settings change. The gate only replaces *when* the existing
  `show_outline` call runs; it never changes what is drawn at a given geometry, so the visual
  result stays pixel-identical.

## Steps (each compiles alone)

- **Step 1 — pure gate (implemented here).** Add `wm/resize_raster.rs` with the gate and four
  unit tests; declare `mod resize_raster;` in `wm/mod.rs`. No caller changed, no behavior change,
  no visual change. Tests: due cadence, 1 kHz cap, latest-geometry trailing flush, cancel clear.
- **Step 2 — wire the live motion path.** Add `outline_raster: OutlineRasterGate<RootRect>` to
  `Wm` (`wm/mod.rs` field, `wm/construct.rs` init); in `effects.rs:231-244` call `note(now, rect)`
  and call `show_outline` only on `RasterDecision::Now`, storing `Wait` geometry as trailing.
- **Step 3 — trailing tick.** From the `wm/event_loop.rs` frame tick call `take_due(now)` and
  present the stored geometry; fold `cap_wait` into the reactor wait so a queued raster wakes the
  loop instead of polling.
- **Step 4 — terminal edges and telemetry.** On `commit.ungrab` / `Cancel`, `clear()` the gate and
  present/hide exactly once so the final outline matches the final geometry; add a
  `record_outline_throttled` counter beside `effects.rs:76`.
- **Step 5 — verification (outside this worker).** Build and run the crate tests with the
  orchestrator's Cargo owner, then scoped runtime resize-trace evidence. Not executed here.

## Reference and choice

- **Path:** `.vendor/xfwm4-master/src/poswin.c:105-145` (`poswinSetPosition`), especially
  `gtk_widget_queue_draw` at `:139`. Secondary: `.vendor/icewm-master/src/movesize.cc:494-524`
  (`outlineResize`), which redraws synchronously on every motion and demonstrates the cost being
  avoided.
- **Choice:** port xfwm4's *semantics* — recompute from newest geometry, queue one coalesced redraw
  per display cadence, always draw the final value — not its GTK code. The cadence is expressed as
  a pure 60/s cap with trailing edge; icewm's per-motion `drawMoveSizeFX` is rejected as the
  freeze mechanism.

## Rollback

Each step reverts independently. Step 1 rollback: delete `wm/resize_raster.rs` and remove the
`mod resize_raster;` line in `wm/mod.rs`. Steps 2-4 revert their own field/call/counter edits;
no settings schema, asset, or visual state is touched at any step.
