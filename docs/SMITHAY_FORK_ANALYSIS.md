# Smithay fork or replacement: performance and memory analysis

Scope: `crates/flamewm-wm-wayland`. Read-only analysis. No source edits, no commits. Smithay is a
registry dependency. `.vendor/smithay-master` is reference material only.

Evidence key: **M** = measured in this run (command and value given). **E** = estimate from code
reading. **U** = unverified (not run, or blocked).

## Verdict

| Option | Verdict | Measured basis (M) | Estimated basis (E) |
| --- | --- | --- | --- |
| Fork smithay | Do not fork | Nested pointer motion costs 23% of one core (M), but that cost is FlameWM repaint policy (`age=0`), not a smithay code path. | A fork would only add maintenance (E). |
| Replace smithay | Do not replace | Smithay-only subtree is 76 crates (M). | Replacement needs Wayland, XWayland and EGL surfaces rewritten (E). |
| Keep and trim features | Keep, already trimmed | `default-features = false`; feature graph shows 6 enabled (M). | Dropping any enabled feature breaks a used module (E). |
| Keep and bypass renderer | Not now | None measured. | Import still needs GL for client buffers (E). |

**Recommendation:** keep smithay 0.7.0 from crates.io with no `[patch]`. The only measured cost that a
fork could touch is the nested full repaint. Its root cause is FlameWM's `age=0` latch, so fix it in
`crates/flamewm-wm-wayland/src/render/mod.rs` before considering any smithay change.

## 1. Usage and features

- M: `crates/flamewm-wm-wayland/Cargo.toml:51`: `smithay = { version = "0.7", default-features = false, features = [backend_winit, backend_egl, renderer_glow, wayland_frontend, desktop, xwayland] }`.
- M: optional `drm` feature (lines 63-72) adds `backend_drm`, `backend_gbm`, `backend_libinput`, `backend_session`, `backend_udev`. Not in default builds.
- M: `Cargo.lock` has `smithay 0.7.0` from the registry. No `[patch]` in `Cargo.toml` or `.cargo/config.toml`.
- M: vendored `.vendor/smithay-master` is also `0.7.0`. Its default features include drm, gbm, libinput, udev, libseat, x11, vulkan (ash) and libei. None are enabled here.
- M: `GlowRenderer` is the only renderer used. No `GlesRenderer`, pixman, multigpu or Vulkan.

| Feature | Enabled | Used by FlameWM | Estimated RSS cost (E) |
| --- | --- | --- | --- |
| `backend_winit` | yes | nested backend | pulls winit, EGL and wayland-client |
| `backend_egl` / `renderer_glow` | yes | GL rendering | glow, libloading |
| `wayland_frontend` | yes | all protocols | wayland-server, protocols |
| `desktop` | yes | surface tree helpers | none |
| `xwayland` | yes | XWayland | encoding_rs, x11rb, scopeguard |
| `drm` stack (`backend_drm/gbm/libinput/session/udev`) | no | tty backend, `drm` feature only | not in default build |
| `backend_vulkan`, `renderer_pixman`, `renderer_multi`, `backend_x11`, `backend_libei` | no | none | not linked |

## 2. Code paths (read from source, E unless marked)

- Damage tracking: `render/mod.rs:429` calls `OutputDamageTracker::render_output(..., age, ...)`. `age` comes from `backend.buffer_age()`, or 0 after a latch (`render/mod.rs` around lines 400-420).
- SHM upload: `vendor gles/mod.rs:975-994` uses `TexSubImage2D` on damage rectangles. Full upload only on a new texture or an empty damage list. Already damage-aware.
- Memory elements: `element/memory.rs:152` allocates zeroed storage. `from_slice` copies (line 167). FlameWM uses it only for cursor bitmaps (`src/cursor.rs:176, 244`), which are cold paths.
- Per-frame allocation in FlameWM (not smithay): `render/mod.rs:185` `build_elements -> Vec<Elem>` allocates per frame.
- Frame callbacks: `render/mod.rs:465-511`, 16 ms throttle, per surface tree. No smithay-side cost found.
- Calloop: `Generic`, `Timer`, `Ping`. No custom wrapper.

## 3. Quantities

| Item | Status | Value | Command / source |
| --- | --- | --- | --- |
| Release binary size | M | 8,118,688 B (7.7 MiB). `text` 7,813,450, `data` 292,929, `bss` 7,712. | `ls -l` and `size` on `/home/linsaftw/.cache/cargo-target/release/flamewm-wayland` (shared target, not built here). Mtime 2026-11-08 13:15. Newer than `render/mod.rs` (2026-11-08 10:40). Other uncommitted sources may differ. |
| Other `flamewm-wayland` artifacts | M | `target-rel/release` 7,789,616 B (2026-10-29). `target-wl/release` 6,950,568 B (2026-10-28). Older. | `ls -l` |
| Normal dependency lines | M | 366 | `cargo tree -p flamewm-wm-wayland -e normal --offline \| wc -l` |
| Unique normal-tree lines | M | 211 (includes features and duplicate versions) | `cargo tree ... --prefix none \| sort -u \| wc -l` |
| Unique normal packages reachable from wm-wayland | M | 174 (incl. root) | `cargo metadata --offline` resolve graph, normal deps only |
| Packages reachable only via smithay | M | 76 (list in the table below) | same graph, with the smithay edge removed |
| Smithay feature graph | M | 6 enabled features as in section 1 | `cargo tree -e features -i smithay --offline` |
| Idle PSS, nested, no client | M | `flamewm-wayland` 77.3 MB PSS, 151 MB RSS. `flamewm-audio-helper` 2.2 MB. `dbus-daemon` 0.6 MB. | `/proc/<pid>/smaps_rollup`, 25 s after start |
| PSS, nested, settings window open | M | `flamewm-wayland` 81.4 MB. Xwayland `:1` 41.4 MB. `flamewm-settings` 25.3 MB. Total own ≈ 151 MB. | same, own PIDs only |
| Idle CPU, nested, no client | M | 0 ticks / 20 s (0.00%) | `/proc/<pid>/stat` utime+stime |
| Idle CPU, nested, settings window | M | 0 ticks / 20 s (0.00%) | same |
| CPU under pointer motion, nested | M | 1107 XTest events / 20 s. `flamewm-wayland` 464 ticks = 23.2% of one core (2000 ticks possible). | `/home/linsaftw/.r115/motion.py` (deleted after run) |
| CPU under pointer motion, nested, xterm client (origin/main a20bb184) | M | 3 runs, 20 s XTEST motion, `/proc/<pid>/stat` utime+stime: 720, 702, 719 ticks (mean 714 = 35.7% of one core). Motion events 1693-1754. Load avg 40-46 on 12 cores. | `/tmp/claude-1000/damage-logs/run_motion.sh`, `motion.py` |
| CPU under pointer motion, nested, no client | M | 20, 22, 21 ticks (about 1%). The repaint cost shows only with a client window. | same |
| PSS / RSS, nested, xterm client, baseline | M | PSS 105.2, 106.1, 105.5 MB; RSS 165 MB (same runs). | `/proc/<pid>/smaps_rollup` |
| Buffer age from the driver, nested winit, after first submit | M | Probe forced on (startup latch disabled, temp build, reverted): frame ages 0, 1, 1, then the query failed and latched 0. Llvmpipe (class software). Ticks 755 (one run), no gain. | `FLAMEWM_AGE_PROBE` log lines |
| Full-repaint cost vs damage-limited | U | Partial repaint needs a trustworthy buffer age. The nested age is inconsistent (row above), so no partial path was built. | n/a |
| drm backend repaint and RSS | U | Not run. Needs a logind seat and tty, not available in this session. | n/a |
| Buffer-age probe outcome | U | `wm.log` shows no EGL or buffer-age lines. The `age=0` latch is confirmed from source only. | `grep -i buffer.age\|EGL wm.log` |
| Gain from removing smithay | E | Unknown. Nothing measured to remove. | n/a |

Caveats on the M values: the host load average was 28-40 on 12 cores during the run. Each number is a
single trial. Software GL (`LIBGL_ALWAYS_SOFTWARE=1`) inflates RSS and CPU. The totals are not comparable with
the existing `docs/PERFORMANCE.md` figures (23.8 MiB session and 26.9 MiB direct, X11 Xephyr stack, 2026-10-30).

### Smithay-only packages (M, 76)

`smithay 0.7.0`, `smithay-client-toolkit 0.19.2`, `calloop 0.13.0`, `calloop-wayland-source 0.3.0`,
`wayland-backend 0.3.17`, `wayland-client 0.31.15`, `wayland-csd-frame 0.3.0`, `wayland-cursor 0.31.14`,
`wayland-egl 0.32.11`, `wayland-protocols 0.32.13`, `wayland-protocols-misc 0.3.12`,
`wayland-protocols-plasma 0.3.12`, `wayland-protocols-wlr 0.3.12`, `wayland-scanner 0.31.11`,
`wayland-server 0.31.14`, `wayland-sys 0.31.11`, `winit 0.30.13`, `glow 0.16.0`, `libloading 0.8.9`,
`encoding_rs 0.8.42`, `x11-dl 2.21.0`, `x11rb 0.13.2`, `x11rb-protocol 0.13.2`, `xkbcommon 0.8.0`,
`xkbcommon-dl 0.4.2`, `xkeysym 0.2.1`, `drm-fourcc 2.2.0`, `as-raw-xcb-connection 1.0.1`,
`cursor-icon 1.2.0`, `dpi 0.1.2`, `raw-window-handle 0.6.2`, `memmap2 0.9.12`, `tempfile 3.27.0`,
`rustix 0.38.44`, `scopeguard 1.2.0`, `quick-xml 0.41.0`, `profiling 1.0.18`, `profiling-procmacros 1.0.18`,
`atomic_float 1.1.0`, `ahash 0.8.12`, `hashbrown 0.17.1`, `indexmap 2.14.2`, `appendlist 1.4.0`,
`approx 0.4.0`, `cgmath 0.18.0`, `core_detect 1.0.0`, `cpufeatures 0.2.17`, `crypto-common 0.1.7`,
`block-buffer 0.10.4`, `digest 0.10.7`, `generic-array 0.14.7`, `typenum 1.20.1`, `sha2 0.10.9`,
`downcast-rs 1.2.1`, `dlib 0.5.3`, `fastrand 2.5.0`, `getrandom 0.3.4`, `getrandom 0.4.3`,
`multiversion_no_op 1.0.0`, `percent-encoding 2.3.2`, `ppv-lite86 0.2.21`, `rand 0.9.5`,
`rand_chacha 0.9.0`, `rand_core 0.9.5`, `scoped-tls 1.0.1`, `serde 1.0.229`, `serde_core 1.0.229`,
`simdutf8 0.1.5`, `smol_str 0.2.2`, `syn 3.0.5`, `thiserror 1.0.69`, `thiserror 2.0.21`,
`thiserror-impl 1.0.69`, `thiserror-impl 2.0.21`, `zerocopy 0.8.63`.

Note: some of these (for example `serde`, `syn`, `rand`) may also be reached by other workspace crates through paths
not removed here. The 76 count is the set that disappears when the smithay edge is removed from the
wm-wayland graph, so it is exact for that root.

## 4. Candidate gains

| Candidate | MB gain | CPU/latency gain | Maintenance | Without fork? | Evidence |
| --- | --- | --- | --- | --- | --- |
| Fork smithay | 0 | 0 | High: forked gles/damage/xwayland plus security tracking | Not needed | M: the smithay path is not the measured cost |
| Replace smithay | Unknown | Unknown | Very high | n/a | E |
| Trim smithay features | Small. All 6 are used. | None | Low | Already done | M |
| Bypass `GlowRenderer` | Unknown | Unknown | High: import, damage, clipping | Yes, but a rewrite | E |
| Fix nested `age=0` (FlameWM side) | Not memory | Possibly the 23% repaint cost under pointer motion, but not measured | Low | Yes | M (cost), E (fix benefit) |
| Remove per-frame `Vec` in `build_elements` | Tiny | Small | Low | Yes | E |
| Investigate Xwayland 41 MB PSS | Up to 41 MB if avoidable | Unknown | Medium | Yes | M (size). Log: "Xwayland glamor: GBM Wayland interfaces not available", so Xwayland runs without glamor. Cause unverified. |
| Own XWayland handling | Unknown | Unknown | High | Not justified | E |

## 5. Prioritized actions (≤5)

1. Instrument buffer age once per session in `crates/flamewm-wm-wayland/src/render/mod.rs` near line 400-430. Log the value the nested winit EGL surface returns. Confirm the `age=0` latch. This settles the `U` item in section 3.
2. Measure partial vs full repaint on the nested session by the same pointer-motion method (20 s, `XTest`). Compare with the 23.2% baseline above. Only then decide whether the fix is worth doing.
3. Investigate the Xwayland 41 MB PSS, with the glamor-unavailable log line as the first lead. Files: `crates/flamewm-wm-wayland/src/main.rs` (XWayland spawn, around the `xwayland` startup stage).
4. Reuse the element buffer in `crates/flamewm-wm-wayland/src/render/mod.rs:185` (`build_elements`). Keep only if step 2 shows a change.
5. Keep smithay on crates.io 0.7.x with no `[patch]`. Record the 174-package and 76-package counts in `docs/PERFORMANCE.md` next to the binary size.

## What was not verified (U)

- drm backend: repaint cost, RSS and whether `buffer_age` returns a real value there. No seat or tty in this session.
- Full-repaint vs damage-limited CPU. Only the full-repaint path was run.
- Whether the nested `EGL_BUFFER_AGE` refusal is driver-specific. The `age=0` latch is confirmed from source, not from the probe's return value.
- Binary size of a fresh build. The measured binary is from the shared target, built 2026-11-08 13:15, and may not match the current tree.
- `docs/AGENT_PLAYBOOK.md` §8 does not exist in this repo. The session followed `docs/PERFORMANCE.md` lines 67-77 instead. Display `:115`, own PIDs only. Scratch config and runtime dirs were removed. `/home/linsaftw/.r115` was deleted after the run.
- Settings window: a second run after launch showed no change in CPU. Its PSS was sampled once.
- Vendored smithay vs registry 0.7.0: only version and feature list were compared, not file contents.
