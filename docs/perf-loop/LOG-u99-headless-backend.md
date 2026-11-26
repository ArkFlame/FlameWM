# u99-headless-backend

Route evidence: read `.agents/skills/no-adhoc-scripts/SKILL.md` and
`.agents/skills/scoped-perf-tests/SKILL.md`, plus `docs/perf-loop/LOG.md` (u94 verdict).

## Question

Does `flamewm-wm-wayland` have, or can it cheaply get, a headless/offscreen render backend (no
`swap_buffers`) so `scripts/compose-scenario` can sample a steady active second?

## Answer: no cheap no-swap backend; the cheap "no host" path is a private Xvfb

Evidence (`.vendor/smithay-master`, 0.7.0, read-only reference):

- Feature list has `backend_winit`, `backend_x11`, `backend_drm`, `backend_gbm`, `backend_udev`,
  `backend_session`; there is no `backend_headless` / offscreen backend.
- `src/backend/winit/mod.rs`: `init` / `init_from_attributes` always build a winit `EventLoop` and
  a window (`EventLoop::builder().build()`, then window attributes). Presentation is
  `WinitGraphicsBackend::submit` -> EGL `swap_buffers`; there is no submit-free variant.
- `src/backend/egl/display.rs`: `EGLDisplay::new(native)` takes a native display handle (winit,
  xcb, or DRM device). Smithay has no surfaceless/pbuffer constructor, so a Glow renderer still
  needs a real display connection; pixman (`renderer_pixman`) renders into memory but still needs a
  backend loop and target, which smithay does not provide for this stack.
- FlameWM's render path is `GlowRenderer`-typed in 61 places across 16 files (colour pass shaders,
  dmabuf import, screencopy readback, `state.backend: Option<WinitGraphicsBackend<GlowRenderer>>`).
  A second renderer/backend would mean generics or a duplicate render pipeline: not cheap.

So no env-gated headless backend was added. The one-shot offscreen path that already exists
(`FLAMEWM_WAYLAND_DUMP` -> `render::dump_offscreen`) is a readback for evidence, not a frame loop:
it allocates and reads back a texture per dump, which would dominate a composition sample.

## Change made (harness, not runtime)

`scripts/compose-scenario --xvfb`: starts a private `Xvfb` (`-displayfd 3`, no python, no fixed
display number), re-runs the script inside it with `WAYLAND_DISPLAY` unset (otherwise winit builds
a Wayland event loop and dies with `Failed to initialize an event loop`), and tears the server down
on exit. The nested compositor still swaps, but the swap is a local framebuffer copy instead of a
blocking host round trip, which is exactly the dilution `LOG.md` recorded for u94.

Precedent: `scripts/break/zoom.sh` already runs the compositor on a private Xvfb with
`unset WAYLAND_DISPLAY`; this reuses that route without its `python3` free-display helper.

Also added a module-doc note in `crates/flamewm-wm-wayland/src/main.rs` recording the same decision
so the next worker does not re-scope a headless backend.

## Composition (one-second ticks, shares of the sampled second)

| Run | render | flush | wayland | wait |
|---|---|---|---|---|
| host session (session `DISPLAY`), same script | 10.9 | 0.3 | 0.0 | 88.7 |
| `--xvfb` | 76.0 | 0.2 | 0.1 | 23.7 |
| `--xvfb` | 60.1 | 0.0 | 0.0 | 39.8 |
| `--xvfb` (earlier pair) | 79.2 | – | 0.0 | 20.8 |
| `--xvfb`, back-to-back loop (CPU-contended) | 9.0 / 13.1 / 11.3 | 0.0 | 0.0 | rest |

Reading: the host path parks 89% of the active second waiting on the host swap; on Xvfb the same
scenario spends 60-79% of the second inside `render` and 21-40% parked. Render is now the dominant
share, and the earlier u94 conclusion ("the nested harness cannot give a steady ACTIVE composition")
is specific to a host-paced window. Do not cite the back-to-back loop pair: consecutive runs on one
machine contend for CPU and understate render.

## What this motivates (next units)

- Rank by render: the compositor's per-frame element rebuild (`render::build_elements_into`),
  backdrop refresh, and full-output damage in the XWayland path, not dispatch.
- Keep measuring with `--xvfb` for repeatable numbers; a bare run still shows host-paced behaviour.

## Risk / limits

Xvfb still presents through `swap_buffers`. A true no-swap backend needs either a smithay headless
backend (absent in 0.7) or a generic renderer pipeline, and would measure a different (offscreen)
presentation cost. Two runs in the same minute vary (60-79% render), so treat the share as an
ordering, not a fixed number.
