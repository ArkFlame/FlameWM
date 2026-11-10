# Idle RSS of flamewm-wayland and Xwayland

> Rows: `wrss-scratch`, `wrss-gpu`, `meas-heap` in `docs/BACKLOG.md`.

Base: origin/main `7c83102c`. Build: `cargo build --profile fast -p flamewm-wm-wayland -p flamewm-desktop -p flamewm-shell -p flamewm-settings --bins`, exit 0 (`BUILD_EXIT=0`, 7m 34s). Host: 12 cores, shared; Xvfb nested session, 20 to 30 s idle. Numbers are not release-grade. Raw data: `/tmp/claude-1000/wlrss-*` (not committed).

## Verdict

Nothing on the Wayland idle path clears the 5 MB PSS bar with a product-side change. The two large costs are environmental or feature-driven.

| Component | PSS | Verdict | Evidence |
|---|---|---|---|
| Mesa llvmpipe (`libLLVM.so.21.1`, `libgallium`, 12 `llvmpipe-N` threads) | 29.1 + 6.7 MB (shared with Xwayland) | ENVIRONMENT | Log: `DRI3 error: Could not get DRI3 device`, `renderer="llvmpipe (LLVM 21.1.8, 256 bits)" class=software`. Xvfb has no DRI3, so EGL falls back to software. A GPU host does not load this path. |
| NVIDIA EGL vendor (`libnvidia-gpucomp`, `libnvidia-eglcore`) | 10.6 + 4.0 MB | INEVITABLE on that driver | Loaded by GLVND EGL vendor probe at winit startup. |
| flamewm-wayland binary (fast profile, 9.3 MB) | 8.0 MB | INEVITABLE for this profile | Release (fat LTO) not built; size not measured. |
| Desktop backdrop capture and blur (`[anon]` 3 x 3.4 MB + 1.8 MB) | about 12 MB | ARCHITECTURAL by design | `render/backdrop.rs` keeps the capture when translucent glass is on (the default). Not changed: transparency must stay. |
| Scratch texture kept after capture (`backdrop.rs (line unpinned)`) | 3.0 to 3.5 MB | ARCHITECTURAL, fixable, below bar | Measured below. Not landed. |
| `[heap]` 23.4 MB + `[anon]` 17.0 MB | 40 MB | UNATTRIBUTED | No heap profiler (`heaptrack`, `valgrind` absent). Needs massif or an allocation hook; not done. |
| SysV shm (Shm frame buffers) | 3.5 MB | UNVERIFIED | Not traced to a client. |
| Threads: 14 `flamewm-wayland`, 12 `llvmpipe`, audio r/w, disk, settings, Shm | 33 total | Llvmpipe threads are ENVIRONMENT | `LP_NUM_THREADS=0` gives 7 threads and -0.8 MB PSS, so threads are not a PSS driver. |

Xwayland (61.6 MB PSS, 114.6 MB RSS):
- Shared with wayland (libLLVM 16.2, gallium 1.9, nvidia EGL 14.6 MB PSS in total). Environment and inevitable, with the process mapping the same files.
- `memfd:xwayland-shared` 14.8 MB PSS: root and shared pixmap memory sized to the output. INEVITABLE for an output of this size.
- Private heap 5.8 MB, anon 2.1 MB: small.
- Launch flag `-glamor none` is applied (confirmed on the live command line) but RSS stays 114.7 MB and PSS 61.8 MB. Its EGL probe still loads the same libraries. No launch change.

## Experiments (wayland idle, PSS in KB, lower is better)

| Variant | Wayland RSS | Wayland PSS | Threads | Result |
|---|---|---|---|---|
| Baseline `7c83102c` (run 1 / run 2) | 165232 / 165700 | 109649 / 109695 | 31 to 33 | reference |
| `LP_NUM_THREADS=0` | 165228 | 109252 | 7 | -0.8 MB, not landed |
| `MALLOC_ARENA_MAX=2` | 165388 | 109483 | 31 | -0.5 MB, not landed |
| `GALLIUM_DRIVER=softpipe` | 190248 | 134561 | 6 | +24.9 MB, worse, rejected |
| Drop scratch texture (`drop(texture)` at `backdrop.rs (line unpinned)`) | 162756 / 162244 | 106767 / 106173 | 31 to 32 | -3.0 to -3.5 MB, below 5 MB, not landed |
| Xwayland `-glamor none` | 162372 | 106319 | 31 | Xwayland unchanged, no launch change |

Baseline Xwayland: 114424 KB RSS, 61381 KB PSS. Discovery figures (`idle60.tsv`): 172.7 MB and 109.8 MB, 120.8 MB and 61.3 MB, on an older build. The new baseline differs by 5 to 7 MB RSS across runs.

## Candidates not landed

- Scratch texture retention (-3.0 to -3.5 MB PSS): real but below the 5 MB bar and it adds allocation on each recapture. Revisit if a larger idle target is needed.
- Desktop backdrop capture at idle (about 12 MB): would be the largest saving, but it removes translucent backdrop on an idle desktop. Forbidden by the task (keep transparency). Only a design change could do it.
- Mesa software fallback (about 36 MB PSS with llvmpipe): not product code. Re-measure on a GPU host before deciding anything.

## Heap attribution (meas-heap)

Base: origin/main `c08cdcc4`, fast profile, private Xvfb `:457` (1600x900), idle 30 s. Method: no perf, no heaptrack or valgrind (absent). Scratch counting `#[global_allocator]` in the three bins, not committed: it tracks live bytes and per-site stacks (`backtrace(3)`) for allocations of 16 KiB or more, dumped every 5 s. Smaps from `/proc/<pid>/smaps` for mappings. Scratch allocator did not change PSS (see the PSS runs below).

| Process | Rust live (global allocator) | Sites >= 16 KiB | Rust live < 16 KiB | `[heap]` touched | Other anon touched |
|---|---|---|---|---|---|
| flamewm-wayland | 0.46 MB | 4 sites, 0.09 MB | 0.44 MB | 10.9 MB | libnvidia-gpucomp 2.6, libLLVM 2.2, libnvidia-eglcore 1.5 MB |
| flamewm-desktop | 10.6 MB | 21 sites, 7.2 MB | 4.7 MB | 8.9 MB | one 3.4 MB mmap (`[anon]`) |
| flamewm-shell | 3.5 MB | 7 sites, 2.2 MB | 1.4 MB | 5.0 MB | none over 1 MB |
| Xwayland | not ours | n/a | n/a | 15.4 MB | shared libs as above |

Top Rust sites (live, resolved by `addr2line` on the fast binary; `??` where the frame has no symbol):

| Subsystem / site | Process | MB live | Count | Note |
|---|---|---|---|---|
| `flamewm_image_core` `Vec::with_capacity` | desktop | 3.38 | 1 | Single buffer, caller not resolved; same size class as the 3.0 to 3.5 MB scratch texture in the Experiments table, likely the same buffer (not confirmed) |
| `flamewm_render_core` `Vec::with_capacity` | desktop | 0.71 | 1 | Caller not resolved |
| `icon_theme::lookup::IconLookupIndex::candidates` | desktop | 1.18 | 5 | Icon candidate lists, icon cache |
| `render::text_shadow::text_shadow_image` | desktop | 0.64 | 23 | Text shadow images |
| `render::text_shadow::text_shadow_image` | shell | 0.47 | 20 | Same path in shell |
| `alloc::vec::from_elem` | shell | 0.98 | 2 | Caller not resolved |
| `image_core::blur::gaussian_blur_rgba_with` | desktop, shell | 0.15 + 0.24 | 3 | Blur scratch |
| `ui_x11::surface_runtime::desktop_backdrop` | shell | 0.21 | 1 | Backdrop |
| `render_x11::xft::text_alpha_mask`, `render_core::glyphs::caret_positions`, `redraw_if_dirty` | desktop | about 0.25 | 6 | Text and frame scratch |

Caller resolution (code reading on origin/main `5077064f`, no runtime trace, not confirmed):

| Buffer | Likely caller | Needed at idle? | Droppable without a visible change? |
|---|---|---|---|
| `image_core` 3.38 MB | `wallpaper::compose_region` (`crates/flamewm-image-core/src/wallpaper.rs:124`), reached from `wallpaper_loader::decode_bytes_at` through `wallpaper_fit::compose` for cropped Fill and tiled fits. The composed raster is held as `WallpaperRaster.pixels` (`Arc<Vec<u8>>`) for the session. The 17.5 MB natural decode of the bundled 2560x1707 WebP is already consumed by `compose_decoded`, so it is not retained. Size match is not proven: a full 1600x900 RGBA canvas is 5.76 MB, so the 3.38 MB buffer implies a different target size or a different buffer. | Yes, while the wallpaper is shown. | No. Freeing it needs the renderer to stop reading the CPU copy after upload, which is not a tiny change. |
| `render_core` 0.71 MB | Candidate `codec.rs:147` (RGB to RGBA asset expansion, kept with the decoded render document). `pixels.rs:152` and `:221` are per-frame region buffers and are transient. | Likely yes for the document assets. | Unknown. |
| `icon_theme::IconLookupIndex::candidates` 1.18 MB | Icon lookup index, generation-scoped, shared by `icon_service`. | Yes for lookups. | Only by evicting cold icon lookups, not measured. |

Verdict: none of the three is a clear dead allocation at idle. The desktop wallpaper decode path already drops its natural decode, so there is no tiny fix of 2 MB or more. No code change. A scratch counting allocator was not run in this pass, so the mapping above is code reading only.

Attribution by owner:
- Wayland `[heap]` 10.9 MB touched, Rust live 0.46 MB: about 10.4 MB is malloc from non-Rust code (Mesa, libLLVM, NVIDIA EGL, libwayland). Not attributable to a Rust site without malloc interposition. ENVIRONMENT, as above.
- Desktop: Rust live 10.6 MB is the only FlameWM-owned heap of size. Its largest single site is 3.4 MB, under the 5 MB bar.
- Shell: 3.5 MB Rust live, no site over 1 MB.
- No single FlameWM site reaches 5 MB PSS. Nothing landed. A combined change across the desktop buffers (3.4 + 0.7 MB) and the icon and text caches would be about 5 MB, but it touches the backdrop and text caches with no measured win, so it stays in the backlog.

Idle PSS runs (KB, Xvfb, unrelated load on host; numbers vary by about 8 MB run to run here):

| Build | Run | flamewm-wayland PSS | desktop PSS | shell PSS | Xwayland PSS |
|---|---|---|---|---|---|
| base | 3 | 46408 | 20952 | 12288 | 49638 |
| base | 4 | 46460 | 20452 | 12506 | 49486 |
| base | 5 | 46614 | 19978 | 12468 | 49529 |
| alloc (scratch, not landed) | 1 | 46598 | 21021 | 12502 | 49429 |
| alloc (scratch, not landed) | 2 | 55163 | 21967 | 13437 | 57288 |

Base runs 1 and 2 ran but their output was lost to a `tail` cut, so only runs 3 to 5 are recorded. The alloc build is not a candidate and is not part of the product.

## Unverified

- Caller of the 3.4 MB `image_core` buffer and of the 0.7 MB `render_core` buffer (needs symbols with inlining or a debug build).
- The 10.4 MB of non-Rust heap in the Wayland process, by library (needs malloc interposition or `malloc_info` per arena).
- The 3.5 MB Shm buffers, traced to a client.
- A release (fat LTO) build, to size the 8 MB binary.
- Any GPU-host measurement (the numbers above all come from software GL).
