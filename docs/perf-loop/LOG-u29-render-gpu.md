# u29-render-gpu changes

Third pass over the pixel paths. Prior units already removed the first-order waste
(retained software frame scratch, borrowed shadow/mask keys, allocation-free text
measure, single-upload GPU batch). This pass targets the copies and allocations
that still sit on the steady per-frame path, ranked by how often the frame hits them.

- render-x11/accel/helper: the GPU batch no longer allocates a whole frame per call.
  `RemoteGpu::paint_batch` used to build a fresh request `Vec` (full-frame copy),
  read the reply into a second fresh `Vec` and copy it out again; the helper child
  did the same on its side (`decode_request` frame copy, `to_vec`, `reply_ok` copy).
  `RemoteGpu` now owns retained `request`/`reply` buffers, `encode_request_into`
  appends into the request buffer, `read_message` fills a caller buffer, and
  `reply_payload` borrows the frame out of the reply instead of copying it. The
  child keeps one message, one frame and one reply buffer for the life of the
  process. Composition shift: on the GPU path (`renderAcceleration=gpu`) every
  composited frame drops four whole-frame heap allocations and two full-frame
  copies; the retained buffers are the same size the transient ones were, so peak
  RAM is unchanged.
- render-x11/app/render/gpu: the whole-frame GPU paint retains its scratch. The
  RGBA8 scene was `vec![0; w*h*4]` per frame and the ARGB32 words were `collect`ed
  into a second full-frame vector. Both now come from retained `gpu_frame_rgba` /
  `gpu_frame_words` on the app, zeroed/truncated in place and given back after the
  upload. Retention is capped at 16 MiB per buffer (the same bound the software
  backend uses), so a 4K desktop still allocates per paint instead of pinning tens
  of megabytes. Composition shift: a steady window paint no longer faults fresh
  pages for two whole-frame buffers per frame, and the word pack grows by `reserve`
  rather than a doubling chain.
- render-x11/backdrop/blur: `bilinear_argb_words` kept a `Vec<Option<Vec<u32>>>`
  table with one line buffer allocated per small row, plus a redundant zero fill of
  the whole line before `horizontal_line` rewrote every element. Neighbouring row
  taps differ by at most one, so two line buffers keyed by source row are enough;
  the line is only resized on a width change. `opaque_argb_words` now allocates its
  word vector at exact size instead of collecting into a growing vector. Composition
  shift: each blurred backdrop per frame drops up to `small_h` heap allocations and
  one full-line memset per computed source row, with bit-identical output.
- render-x11/app/render, app/render/paint_full, app/render/paint_region: panic paths
  removed from the frame path. The three `expect("layout retained above")` /
  `expect("paint cached above")` invariants now return typed `Err` strings, and the
  `unreachable!` in `bilinear_argb_words` is gone with the table it guarded. The
  dead allocating wrappers (`RemoteGpu::call`, the old `read_message`/`parse_reply`
  shims, `encode_request`) are removed or confined to tests.

## Verification

Static reading plus `rustfmt --edition 2024 --check` on every edited file (parses
and is format-clean). No cargo, no git in the shared checkout: compile and tests are
the orchestrator's step.

## Skipped

- Blur target pooling in `accel/gles` (retaining GL targets instead of creating and
  releasing textures per pass) needs interior mutability across `&self` pass code
  and a release contract; left for a unit that can build and run it.
- XImage staging reuse in `app/images`: the transient upload buffer is deliberately
  bounded per call, so retaining it would change the documented memory behaviour
  without a measurable region to blame.
