# u57-imaging

Area: `flamewm-image-core`, `flamewm-imageviewer`, `flamewm-screenshot` — decode/scale pipeline memory.
`git` and `cargo` are forbidden in this shared checkout, so the review of the earlier slices is source-based (the
batch logs u15/u40/u49/u50 plus the current code) and no build was run here: the orchestrator builds, tests and
commits.

## Review of the landed slices

- **u15 (imageviewer/archiver/image-core).** All five changes are present and intact: the coalesced progress
  reporter, the 64 KiB tracked copy, the per-directory `prepare_parent`, the image-viewer column map in `crop`,
  `read_entry`'s `drain(..HEADER_LEN)`, and `TapTable`. The column map still did 4-byte `extend_from_slice` per
  pixel, which the original fix left behind; that is the first item below. No behavioural regression found.
- **u40 (misc apps).** The finder `paint_duplicates` bounds fix and the watcher/index changes are outside this unit
  and read as described. Nothing in the image-viewer state machine relies on the old `reduced` lifetime; the change
  below keeps `request_view`'s contract that a frame is only placed for its own sequence.
- **u49/u50 (assets/RAM, manifests).** `opt-level = 3` still covers `flamewm-image-core` while the viewer binary is
  `opt-level = "s"`; no dependency edge shrank into these crates. Nothing to re-check beyond the code below.
- One defect-shaped leftover was found in each of three places and fixed here (items 2, 4 and 6): each staged a second
  full-size image buffer only to hand it to a writer that could have consumed it directly.

## Next worst hotspots by composition share

1. **Pan/fit frames in the image viewer (`imageviewer/decode.rs`, `crop`).** Every viewport request copies the
   visible rect (up to the stage, ~1000x680) out of a same-size base. At fit zoom the mapping is identity, so the
   old loop still paid one `Vec::extend_from_slice(&[u8; 4])` — a capacity check and a 4-byte `memcpy` — per pixel.
   `crop` now walks the column mapping once into runs of adjacent source columns and copies each run into a
   preallocated output. Identity and zoom-out crops collapse to one row `memcpy` per row; a nearest-neighbour
   upscale collapses to one run per source column. Pixel values, nearest-neighbour mapping and the frozen crop tests
   are unchanged.
2. **The reduced-copy hand-off (`imageviewer/decode.rs`, `view`).** A full-rect request at zoom-out scaled the image
   to `display`, cached it, then copied it again through `crop` into the reply: two display-sized buffers written per
   frame. A full rect means the image fits the stage on both axes (`place` clamps to the stage), so no pan can follow
   and the cache slot would only pin a second buffer. The scaled raster's pixels now go straight into the reply; a
   still-fresh cached reduction is moved out with `Arc::try_unwrap` instead of cloned. Frame size and placement are
   the same, so the window sees an identical `Reply::Frame`.
3. **Wallpaper composition taps (`image-core/wallpaper.rs`, `emit_region`).** The area-composition path still built
   one `(usize, Vec<f32>)` per visible column and per visible row — a full-HD compose allocated ~4000 short vectors
   and then read them through `Vec` indirection for every output pixel. It now uses `scale::TapTable` for both axes
   (three flat allocations per axis) through `tap_at`, which keeps the letterbox `BACKGROUND` rule for destinations
   outside the layer. `push_area_tap` is the same arithmetic, so the golden wallpaper output and the odd-ratio pixel
   tests are unchanged. `area_tap` had no remaining caller and was deleted; the coverage-weight tests now call the
   shared `push_area_tap` through a test-local helper, so the contract is still exercised.
4. **Decoded-wallpaper cache write (`image-core/decode_cache.rs`, `write_entry`).** The entry was assembled as
   `header + pixels` in a fresh `Vec` and handed to `fs::write`, so writing a 4K raster (33 MiB) held two copies at
   peak. The header and the image bytes are now streamed into the temp file through one `File`, and the temp file is
   removed on a write failure instead of being left behind. The on-disk layout, keying, prune order and
   never-evict-the-new-entry rule are untouched.
5. **Wayland screencopy conversion (`screenshot/wayland.rs`).** `xrgb_to_rgba` allocated a row-pointer `Vec`, then a
   second full frame, then pushed 4 bytes per pixel. Screen recording calls this at the capture rate, so the copy was
   per-frame overhead on top of the read. The compositor's buffer is now converted in place (`B<->R` swap, alpha set,
   rows swapped in place only when `YInvert` is set): one frame buffer alive per capture and no row-index `Vec`. The
   size-mismatch contract stays (`false`, buffer untouched); the former allocating form survives as a test-only
   wrapper so the pixel/row-order tests still run.
6. **General `XImage` decode (`screenshot/capture.rs`).** The 32-bpp fast path had hoisted its per-channel arithmetic
   but the general 16/24-bpp path still ran `component`'s two 64-bit divisions per channel per pixel and grew the
   output with a per-pixel `extend_from_slice`. Both now share one `Channels` helper (8-bit channels are a shift and
   mask, narrower/wider ones keep the exact `component` rounding) and the output is allocated once and written
   through `chunks_exact_mut`. The 24-bpp root visual, which is 8-bit per channel, no longer divides at all.
7. **Overlay pixel packing (`screenshot/ui.rs`, `pack_rows`).** Each overlay setup packed the whole screen with three
   `pack_color` calls per pixel; every call recomputed `trailing_zeros`, `max`, a multiply and a divide. The mask
   shifts and the "is this channel exactly 8-bit" test are now computed once per batch, and an exact 8-bit channel
   packs with a shift instead of the scale/divide. The emitted bytes for the general 16-bpp and non-standard-mask
   cases are unchanged (those keep calling `pack_color`).

Deleted: `scale::area_tap` (no non-test caller after item 3); its coverage contract moved into the test module.
Nothing else was orphaned — `TapTable` is now crate-visible for the wallpaper path and `component` is still the
general-channel implementation used by both callers and the 565 tests.

Expected composition shift: image-viewer pan/fit frames stop doing per-pixel copy calls and fit frames stop writing a
second display-sized buffer; wallpaper compose stops allocating thousands of per-tap vectors; the decoded-wallpaper
cache write halves its peak raster copies; each Wayland capture and each X11 overlay setup does one fewer full-frame
allocation/copy and far fewer per-pixel divisions. Visual output, frame sizes, row order, pixel values and error
kinds are unchanged.
