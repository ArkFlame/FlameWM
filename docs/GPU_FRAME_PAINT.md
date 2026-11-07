# GPU frame paint (`renderAcceleration`, default `cpu`)

## Status and default

- Default stays `cpu`. GPU is opt-in: setting `renderAcceleration=gpu`, or env `FLAMEWM_GPU=1`.
  Env `FLAMEWM_GPU=0` forces the CPU path and wins over the setting.
- Decision (2026-11-07): the default flip to `gpu` was reverted. Measured on the RTX 3050 Ti:
  GPU-on adds about 143 MB PSS to the desktop process (168.9 MB vs 25.7 MB), and GPU-on live use
  only engages the GPU in the desktop process. The WM, explorer, terminal and settings never load
  EGL. The GPU gain is limited to blur and `paint_batch`.
- Per-op fills and strokes are routed to the CPU in every mode. Every GPU call pays a full upload and
  readback, so a per-op GPU fill measured 2.5x to 170x slower than the CPU.
- GPU engages for: backdrop blur (`accel::blur_rgba` / `gpu_blur_rgba`), `paint_batch` (one upload,
  one readback), and the whole-frame path in `app/render/paint_full.rs` (only when every command is a
  rounded fill, stroke or scrollbar track on whole pixels).
- Still CPU (XRender) in every mode: text, images, text shadows, clips, node backdrops, scrollbar
  thumbs, fractional geometry, partial repaints.

## Fallback rules (`accel/mod.rs::try_gpu`)

- GPU init fails (EGL/GLES load, context, shader): one `eprintln` line, CPU path for the rest of the
  process. Retried only after `set_render_acceleration` changes mode.
- Software GL renderer (renderer string contains `llvmpipe`, `softpipe` or `swrast`): treated as an
  init failure, one log line, CPU path.
- Any GL error (`glGetError` checked after each stage, including context loss) or op error: one log
  line, slot becomes `Failed`, the op runs on the CPU. The GPU contract leaves the frame untouched on
  error, so the CPU sees the original pixels. No panic path.
- Whole-frame GPU failure returns to XRender before presenting, so a frame never mixes painters.

## Measured evidence (2026-11-07, NVIDIA GeForce RTX 3050 Ti Laptop GPU, real GL, not llvmpipe)

`cargo run --release -p flamewm-render-x11 --example pixel_bench`, medians (ms):

| Op | CPU | GPU (before per-op fix) | Verdict |
|---|---|---|---|
| blur r12 | 34.8 | 10.8 | GPU win |
| blur r28 800x500 | 13.4 | 6.1 | GPU win |
| fill rounded 600x300 | 3.9 | 9.7 | CPU (now routed to CPU) |
| stroke rounded | 0.06 | 10.4 | CPU (now routed to CPU) |
| selection fill+stroke | 1.6 | 19.8 | CPU (now routed to CPU) |
| move outline | 0.14 | 9.0 | CPU (now routed to CPU) |
| frame batch per-op | 85.6 | 732.7 | CPU (now routed to CPU) |
| frame batch `paint_batch` | 87.8 | 14.7 | GPU win |
| batch blurs only (2b) | 78.0 | 12.3 | GPU win |

Live session, display :96 (Xvfb), 15 s `/proc` probes, GPU on (`FLAMEWM_GPU=1`) vs off (`FLAMEWM_GPU=0`):

| Process | PSS on | PSS off | CPU ticks/15 s on/off | ctxt switches/15 s on/off |
|---|---|---|---|---|
| flamewm-desktop | 168,867 kB | 25,670 kB | 0 / 0 | 0 / 3 |
| flamewm (WM) | 19,409 kB | 19,223 kB | 0 / 0 | 1 / 4 |
| flamewm-explorer | 18,754 kB | 18,791 kB | 0 / 1 | 0 / 1 |

Idle wakeups are equal (near zero). The RAM cost is the desktop EGL/driver load (+143 MB PSS). This
fails "no extra RAM", so the default stays `cpu`. Per-frame ms for desktop+shell were not measured
live (the shell does not run, see blockers).

Visual: root screenshots GPU on vs GPU off (`shot_on_full.png` and `shot_off_full.png` in
`/tmp/claude-1000/fw-g/`) differ by 72 AE pixels of 1.44 M, all in the 9x11 explorer timestamp
("Today, 11:07"). Terminal, settings and explorer render identically in both modes (they never load
the GPU).

## Blockers (not fixed here)

- Shell does not start: `GetShellBootstrap failed: snapshot missing occupancy`. The `occupancy`
  key is always written by `flamewm-control-wire/src/encode.rs` (`workspace_dict`), so the likely
  loss is the D-Bus layer dropping an empty `occupancy` array (`flamewm-control-dbus`, commit
  f7fd3a09). This blocks the shell-panel review. Out of this task's scope.
- `flamewm-session` cannot run beside a standalone `flamewm` (XSettings already owned), so the
  harness runs WM, desktop, explorer and apps directly.
- Explorer, terminal and settings do not call `apply_render_acceleration`, so the setting is inert
  in those processes. The GPU is reachable only through the desktop and shell.

## Not verified

- GL leak soak (texture/FBO create vs free over many resizes): static audit only. Per-op targets are
  released after each op (`gles.rs::release`), and `GpuBlur::drop` frees the program, quad and
  context. No runtime soak was run.
- KHR_debug / debug-context GL error capture: not run. Errors are checked by `glGetError` after
  each stage.
- Live review of popups, notification centre, snap preview and resize/drag under GPU on/off:
  not run. Only the desktop, explorer, terminal and settings were captured.
- Parity for text, images, clips and shadows: these never run on GPU, so the switch does not change
  them.

## Tests

- `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-g CARGO_BUILD_JOBS=3 RUST_TEST_THREADS=2 cargo test -p flamewm-render-x11 -p flamewm-platform -p flamewm-ui-x11`: exit 0, 751 passed, 0 failed, 0 warnings. Includes `env_override_wins_only_for_zero_or_one_and_software_gl_is_detected`
  and `render_acceleration_defaults_to_cpu_and_accepts_only_cpu_or_gpu`.
- `cargo test -p flamewm-render-x11 --lib accel`: exit 0, 21 passed. Measured GPU vs CPU/XRender:
  rounded fill/stroke max 8/255, mean 0.029; blend max 1, mean 0.0005; blur mean difference 0.17 to 0.42.

## Contract

- `flamewm-render-core::PixelOp` is one step of a frame batch: `FillRoundedRect` (a stroke is a
  fill with a hole) or `Blur`. Ops are applied in order to one RGBA8 frame.
- `PixelBackend::paint_batch` applies a batch. The default implementation runs each op on a
  scratch copy and copies back only on success, so an error leaves the frame untouched.
- `flamewm-render-x11::paint_batch` is the single selection point: GPU when `renderAcceleration=gpu`
  and the GPU is healthy, otherwise the CPU batch. A GPU error falls back to the CPU.

## GPU batch

One texture upload, then every op runs on the frame-resident render target: fills draw the
rounded-rect shader, blurs ping-pong through intermediate targets and replace the frame target.
One readback returns the frame. Blur uses the same sigma mapping as `blur_rgba` on the GPU.

## Whole-frame path (`app/render.rs`, `app/gpu_frame.rs`)

- The rounded-rect shader uses a one-pixel signed-distance ramp, the same edge model as the
  XRender AA path. Corner pixels match `xrender_aa` within 8/255 per channel (mean under 0.03).
- `paint_full` tries the GPU frame first. It runs only when `renderAcceleration=gpu`, the scene
  has an XRender backbuffer, no backdrop or clip is active, and every command is a rounded fill,
  a stroke or a scrollbar track on whole device pixels. Any other command keeps the XRender path
  wholesale.
- The GPU result is premultiplied and uploaded once to the backbuffer. A GPU failure returns to
  the XRender path before anything is presented.
- Partial repaints (`paint_region_commands`) stay on XRender.
