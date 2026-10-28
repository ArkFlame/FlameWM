# FlameWM 0.1.3 finish-line verification — STATUS: INCOMPLETE

Not 100%. A5 (X11 GPU provider) is not implemented; several measurements were not taken.

| Req | Status | Evidence |
|---|---|---|
| A1 build/E0432 | PARTIAL-PASS | E0432 did not reproduce on the live tree (isolated CARGO_TARGET_DIR). Root cause: `crates/flamewm-api/src/graphics.rs` was untracked in git; now committed. Regression test `crates/flamewm-render-x11/tests/graphics_api_import.rs` passes. `cargo check --locked --workspace --all-targets` and `-p flamewm-wm-wayland --features drm` pass. Workspace tests: stale default-animation tests repaired; `flamewm-profiler process_cpu_clock_counts_other_threads` (timing/load sensitive) still fails on this box, `foreign_event_livelock` is flaky under full-workspace load (passes alone). `cargo clippy` is not installed here: NOT RUN. |
| A2 dependencies | PARTIAL | Per-package unique resolved counts (cargo tree, normal+build): wm-wayland 163, shell 107, wm-x11 111. Nothing removed; no unproven removal claimed. Added `fontdue` (+ttf-parser, core_maths, libm) to wm-wayland only. PSS/size/startup: NOT MEASURED. |
| A3 audio | PASS (feature wiring) / UNVERIFIED (live) | libpulse-binding is linked only into wm-x11 (opt-in `pulse-audio`); wm-wayland and shell do not link it. `cargo test -p flamewm-integrations-linux --features pulse-audio`: all pass. No live PulseAudio/PipeWire-Pulse control test (`pactl` absent). |
| A4 shared chrome | PARTIAL | Wayland titlebar now uses IBM Plex Sans via fontdue (cached glyphs, Unicode, elision, clipped to title region); unit test asserts no title pixels in controls strip; rendered sample inspected visually. `elide_title` fixed for widths where `...` cannot fit. Not done: side-by-side Xephyr vs nested Wayland screenshots, hover/pressed state pixel checks on a live session. |
| A5 X11 GPU | NOT IMPLEMENTED | `GraphicsProvider` is selection-only; no EGL/GLES X11 paint backend exists. |
| A5 Wayland GPU | PARTIAL | Nested winit run on this host logs `FLAMEWM_GL_RENDERER renderer="Mesa Intel(R) UHD Graphics (TGL GT1)" class=hardware` (new, also wired for DRM). DRM compiled only (`--features drm`); no TTY/seat run. XWayland fell back to software glamor (`GBM Wayland interfaces not available`). |
| A6/A7 | PARTIAL | `./scripts/package-source-archive --check` passes (stray exec bits on svgs removed, tools registered). Pristine-extract build check recorded by the release step. Xephyr/`./scripts/wayland` full runs and `verify-visual`: NOT RUN. |

Skipped: `./scripts/check`, `./scripts/xephyr`, `./scripts/verify-visual`, release-profile builds, PSS/VRAM/CPU benchmarks.
