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

## Unverified

- A heap profile to attribute the 40 MB heap and anon, and the 3.5 MB Shm buffers.
- A release (fat LTO) build, to size the 8 MB binary.
- Any GPU-host measurement (the numbers above all come from software GL).
