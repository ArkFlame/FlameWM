# Settings slider drag: WM wake attribution (2026-11-05)

Status: attribution complete. No code change kept. The candidate fixes below either already exist or
cannot reach the >=3% bar without a visual or RAM trade.

## Method
- Base: `origin/main` 6d0caa60 in worktree `/run/media/linsaftw/SDD/wt-settings-wakes`, release build,
  private `CARGO_TARGET_DIR`. Binaries copied to `/run/media/linsaftw/SDD/bin-settings-base`.
- Driver: `attr_run.py` (scratch, same bootstrap as `scripts/perf/settings_session.sh`). Idle 5 s, then 150
  slider motions on host pixels `588,768,297` (`--slider`). Per-thread wakes = voluntary plus nonvoluntary
  context switches from `/proc/<pid>/task/*/status`.
- Driver fix: the first run recorded the `setsid` wrapper's pgid (0% CPU for everything). The
  driver now uses the spawned pid as the session pgid. Earlier numbers from that broken run are not used.
- `perf record` is refused (`perf_event_paranoid=4`). Attribution used /proc only, plus `strace -f`
  on the sends and a temporary frame counter (reverted, not committed).

## Result: where the wakes are (base, drag window 2.54 s, 150 motions)

| Thread group | Wakes/s | CPU % |
| --- | ---: | ---: |
| `flamewm-wayland` total | 6663.7 | 48.08 |
| llvmpipe worker threads (12 threads, each 486 to 514 wakes/s) | ~6000 | ~45 |
| `flamewm-wayland` main thread | 683.9 | 3.15 |
| `Xwayland` | 95.0 | 1.58 |
| `flamewm-setting` | 95.0 | 1.58 |

Source: `scratch-settings-cpu/attr-base/20261105-130329-3801760/attribution.json`.
The earlier 10,788 wakes/s figure came from `settings_session.sh` on the same base. The traced run below
measured 9,527 wakes/s and 71.8% CPU, so treat the absolute values as load-dependent.

Frame count, traced build (temporary `FRAME_TRACE` eprintln on `render_frame`, reverted):
- 85 frames in the 2.5 s drag window, about 34 fps. Whole session 101 frames.
- 71.8% CPU over 2.54 s, so about 21 ms CPU per frame.
- Desktop recaptures during the same drag: 3 `BACKDROP_PHASE` lines for the whole session (run with
  `FLAMEWM_BACKDROP_PROFILE=1`). Recapture is not a wake source here.

## Suspects, checked

| Suspect | Evidence | Verdict |
| --- | --- | --- |
| Commit-driven `request_full_redraw` (`state.rs:779`, `handlers.rs:120-149`) | Every settings motion calls `request_redraw`, but `request_winit_frame` (`state.rs:884`) caps frames to one per 16 ms. Commits do not produce extra frames. | Already coalesced to the frame cadence. |
| winit frame timer (`state.rs:884`) | Timer armed at most once per interval. Measured frame rate is about 34 fps, below the 60 fps cap, so the timer is not the limit. | Pacing is render-bound, not timer-bound. |
| Desktop recapture wake (`render/backdrop.rs:152`) | 3 captures per session, each about 9.4 ms median. | Negligible for this drag. |
| Per-frame full software repaint | `BUFFER_AGE` is refused by the nested winit EGL surface (`render/mod.rs` comment), so `damage_render` repaints the full output every frame under llvmpipe. | Root cause of the wake volume. Each frame wakes the llvmpipe pool. |

## Candidate fix rejected: damage-only repaint with a retained full-output texture
- Would skip the untouched output region on each frame, but needs a retained full-output texture (RAM).
- `render/mod.rs` records a prior measurement where a retained texture plus full blit was slower under
  llvmpipe (142% vs 89% marquee CPU, one run each). That was not re-measured here, and this change is a
  renderer change, not a wake fix. Not implemented.

## Not run
- `scripts/perf/fm.sh` A/B for this drag: not run. No candidate reached the bar.
- Per-thread attribution is from /proc samples. `perf` and `strace` counts are indicative only.

## Verification
- `python3 -I tools/architecture_guard.py`: exit 0 (`OK architecture boundary guard (77 ratified violations)`).
- `cargo check --workspace --all-targets`: see the final report.
