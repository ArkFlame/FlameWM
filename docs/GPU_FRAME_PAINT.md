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

## Status

- Stage 3 (routing `app/render.rs` through the batch) is not wired. The canonical CPU scene is
  XRender with anti-aliased corners, and the GPU shader is hard-edged. Routing frames through the
  GPU would change visible corners, so parity needs a decision first.
- Measurements: `cargo run --release -p flamewm-render-x11 --example pixel_bench` (1350x641,
  Mesa Intel UHD). The frame batch is 60 fills, 20 strokes and 2 blurs.
