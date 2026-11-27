# Unit p65-gpu-helper-lazy-rss
STATUS: PARTIAL

Crate flamewm-gpu-helper (142 MB RSS idle, top baseline RSS row B3): find what it allocates at start (rg 'Vec::with_capacity|vec!\[|Buffer|create_' ), make buffers lazy or sized to the actual output. Pixel output identical. Max 3 edits.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-render-x11/src/accel/gles.rs:47-64,67-82,172-210,213-237` | `GpuBlur::new()` at helper start loads libEGL/libGLESv2 and the Mesa/llvmpipe driver, opens the EGL display/context/pbuffer (`eglCreateContext`, `eglCreatePbufferSurface`) and builds six shader programs plus the quad VBO (`gen_buffers`/`buffer_data`). The `create_*` hits are native EGL/GL handles and driver mappings, not Rust `Vec`s; they dominate the 142 MB idle RSS. `docs/PERF_HOTLIST.md:35` (row 14) already classifies the llvmpipe mapping as environmental. | NONE - deferring the EGL context past the first-reply handshake would change the device-name reply contract read by `RemoteGpu::spawn`, beyond one safe pass. | UNBUILT |
| `crates/flamewm-render-x11/src/accel/helper.rs:211-215,223,234-254` | The helper held three full-frame Vecs at once (`body` request, `frame` staging copy, `reply` output), plus `GpuBlur::readback`: four frame-sized buffers live for the helper's lifetime. | Removed the redundant `frame` staging Vec; `serve_one` paints straight into `reply[1..]` and rewrites `reply` as status 1 plus error text on GPU failure. `reply` is resized to exactly one status byte plus the request frame length each call. Same bytes, same ops, same GPU backend contract, so pixels are identical. | UNBUILT |
| `crates/flamewm-render-x11/src/accel/gles.rs:191-210,470-473`; `crates/flamewm-render-x11/src/bin/flamewm-gpu-helper.rs:4-9` | `GpuBlur.readback` starts as `Vec::new()` and `staging()` reuses it at the exact frame length; the child binary allocates no buffer of its own. | NONE - already lazy and sized to the actual output. | UNBUILT |
| `crates/flamewm-render-x11/src/accel/helper.rs:67-74,198-232`; `crates/flamewm-render-x11/src/accel/mod.rs:370-413,436-441` | Parent `RemoteGpu.request/reply` and the helper's `body/reply` all start empty; `GpuSlot::Untried` already delays spawning the helper until the first GPU op, so there is no startup helper allocation left to defer. | NONE - already landed. | UNBUILT |
| `.vendor/smithay-master/src/backend/renderer/gles/mod.rs:575-599` (read only); `third_party/smithay/benches/geometry.rs:6-9` | Reference-first: smithay builds its GL renderer eagerly at construction and reuses a retained `Vec` workhouse across frames; neither KDE nor xfce offers a closer GPU-helper buffer precedent. | NONE - no vendor precedent for a smaller in-scope cut. | None |

Production edits: 1 (helper.rs). No tests were edited; none were requested. Reference-first check done; no upstream clone needed. All findings are static: no build, test, run or RSS measurement was permitted under the worker contract, so the production edit is UNBUILT.

PARTIAL - the startup allocation audit is complete and one safe cut landed: the helper no longer keeps a separate full-frame staging `frame` Vec alongside `reply`, so a request is painted directly into the reply payload and the child retains one frame-sized wire buffer instead of two; pixel output is unchanged. The remaining ~142 MB idle RSS is dominated by the EGL/llvmpipe driver context and shader objects created in `GpuBlur::new()`, which cannot be deferred without changing the first-reply device handshake, and the parent already lazy-spawns the helper on first GPU use. Nothing was built, tested or measured under the worker contract, so the RSS benefit is a static structural claim and the edit is marked UNBUILT.
