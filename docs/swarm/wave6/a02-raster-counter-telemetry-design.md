# a02 raster counter telemetry — design

Status: UNBUILT (no cargo, no build, no tests run by this worker).

## Problem

The resize check needs the number of `show_outline` rasters per second. The WM
already has a debug/telemetry facility: `flamewm_debug::emit` gates message
construction behind `FLAMEWM_DEBUG=1` and a per-event cooldown, and
`counters.rs` `emit_resize_summary()` publishes the resize counter set through
`WM_RESIZE_SUMMARY` every second. `effects.rs` presents every outline through
three `snap_preview.show_outline` call sites (ungrab tail, drag gate, trailing
flush).

## Change (three production files)

1. `crates/flamewm-debug/src/lib.rs`: make the existing `enabled()` gate public
   so telemetry producers can skip work with the same resolved env seam.
2. `crates/flamewm-wm-x11/src/wm/counters.rs`: add `RasterRate` to
   `ResizeCounterSet`. `record()` bumps an `AtomicU64` only when
   `flamewm_debug::enabled()`, so the hot raster path costs nothing when
   disabled (no profiler point, no lock). `sample()` is called only by the
   already rate-limited summary emitter; it swaps the pending count to zero and
   normalizes `count / elapsed` to one second via the pure `per_second()`
   helper. The summary line gains `rasters_per_sec=<n>` after `max_batch`.
3. `crates/flamewm-wm-x11/src/wm/effects.rs`: one private
   `show_outline_raster()` method records and then presents; all three
   `show_outline` call sites now route through it, so a raster is counted
   exactly once regardless of which path produced it.

Tests in `counters.rs` cover the pure math (1 s, 0.5 s, 2 s, 1.5 s, zero
length, zero count) and window draining (a sample cannot double-count into the
next window). No visuals or X behavior change: the same geometry reaches the
same `show_outline` call; only an atomic bump is added on the debug path.

## Reference-first record

- `.vendor/kwin-master/src/compositor.cpp:800` and `:833-835`: KWin derives a
  per-second quantity from a per-frame change using the frame duration
  (`changePerSecond * refreshDuration / 1e9`). Choice ported: normalize an
  observed count by the measured window duration in `per_second()`, rather than
  assuming a fixed 1 s sample. The `kwin-master` corpus does not include the
  separate `showfps` effect source, so there is no stronger per-second frame
  counter precedent here; this is the only available KWin signal.
- The existing in-repo precedent `.vendor/xfwm4-master/src/poswin.c`
  (documented in `wm/resize_raster.rs`) remains the authority for the raster
  gate itself and was not changed.

## Risks

- Sample interval is the summary cooldown (nominally 1 s); a burst straddling
  two windows is split across two lines, which is intended for a per-second
  rate.
- `flamewm_debug::enabled()` can be true before process registration; the
  summary emitter still suppresses the line until `init_process` ran, so an
  enabled-but-unregistered process may accumulate an unread count.

## Verification

Not run by this worker (brief forbids cargo/builds). Orchestrator must run the
`flamewm-wm-x11` unit tests and a workspace check; all edits are UNBUILT.
