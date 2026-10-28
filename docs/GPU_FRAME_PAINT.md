# GPU frame paint (opt-in, `renderAcceleration=gpu`)

## Contract

- `flamewm-render-core::PixelOp` is one step of a frame batch: `FillRoundedRect` (a stroke is a
  fill with a hole) or `Blur`. Ops are applied in order to one RGBA8 frame.
- `PixelBackend::paint_batch` applies a batch. The default implementation runs each op on a
  scratch copy and copies back only on success, so an error leaves the frame untouched.
- `flamewm-render-x11::paint_batch` is the single selection point: GPU when
  `renderAcceleration=gpu` and the GPU is healthy, otherwise the CPU batch. A GPU error falls back
  to the CPU through the same `try_gpu` path as the single-op calls.

## GPU batch

One texture upload, then every op runs on the frame-resident render target: fills draw the
rounded-rect shader, blurs ping-pong through intermediate targets and replace the frame target.
One readback returns the frame. Blur uses the same sigma mapping as `blur_rgba` on the GPU.

## Whole-frame path (`app/render.rs`, `app/gpu_frame.rs`)

- The rounded-rect shader uses a one-pixel signed-distance ramp, the same edge model as the
  XRender AA path. Corner pixels match `xrender_aa` within 8/255 per channel (mean under 0.03).
- `paint_full` tries the GPU frame first. It runs only when `renderAcceleration=gpu`, the scene
  has an XRender backbuffer, no backdrop or clip is active, and every command is a rounded fill,
  a stroke or a scrollbar track on whole device pixels. Any other command (text, images, shadows,
  node backdrops, clips, scrollbar thumbs, fractional geometry) keeps the XRender path wholesale.
- The GPU result is premultiplied and uploaded once to the backbuffer. A GPU failure returns to
  the XRender path before anything is presented, so a frame never mixes the two painters.
- Partial repaints (`paint_region_commands`) stay on XRender.

## Measurements

`cargo run --release -p flamewm-render-x11 --example pixel_bench` at 1350x641, Mesa Intel UHD.
Frame batch = 60 fills, 20 strokes, 2 full-frame blurs. Medians of 25 runs.

| Path | Time |
|---|---|
| CPU per-op | 107.4 ms |
| CPU `paint_batch` | 108.7 ms |
| GPU per-op | 609.0 ms |
| GPU `paint_batch` | 53.3 ms |
| GPU fills and strokes only | 39.5 ms |
| GPU blurs only (2) | 10.9 ms |

The blur is about 20% of the GPU batch. Fills and strokes dominate it, so the cost is the
per-command draws rather than the blur passes.
