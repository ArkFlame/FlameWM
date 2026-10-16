# Changelog

## 0.1.0

### Product and versioning

- Product renamed to FlameWM; the Rust WebRender branch name is no longer used for the product.
- Version is sourced from the single `FLAMEWM_VERSION` file.

### Fixes

- NetworkManager SaveSecrets/DeleteSecrets and unknown SecretAgent members are answered instead of failing; empty secret hints are never sent.
- Keep the NetworkManager batch going when a single event fails.

### Refactors and performance

- Shared helpers (D-Bus message items, reconnect backoff, rounded corner geometry, X11 helper bodies, surface fill) replace duplicated code.
- Micro-optimizations: reused slot id buffers in shell projection, reused blur and backdrop scratch buffers, and fewer per-line allocations in render paths.

## 0.0.9 — 2026-09-24

### Platform and desktop integration

- Added typed settings/control transactions across platform services, D-Bus transport, and the Settings application.
- Added desktop wallpaper loading and portal-backed file selection for desktop and Settings workflows.
- Connected workspace synchronization and reactor presentation scenarios to event-driven desktop updates and verification.
- Improved snap-preview lifecycle and settings/shell projection behavior.

### Documentation

- Rewrote `README.md` for the 0.0.9 X11-only product: current crate layout (`crates/flamewm-wm` ships the `flamewm` binary on top of `crates/flamewm-wm-x11`), verified scripts, default shortcuts, configuration locations, and install/upgrade/uninstall steps (not tested).
- Benchmarks are pending measured runs via `tools/flamewm_perf_compare.py`; no benchmark figures are published.
- Recorded the license metadata mismatch (`LGPL-2.0-only` workspace versus `MIT` for `crates/flamewm-render-compiler`) as pending maintainer confirmation.

## 0.0.8 — 2026-09-09

### Interaction and performance convergence

- Desktop drag ghost follows pointer; canonical entries stay put until release commits existing grid/group semantics.
- Context menus use borderless rows with 16px Breeze icons; pointer menus flip upward and scroll when oversized.
- Rename is Linux no-clobber (`renameat2` NOREPLACE); grid position migrates; collisions fail without data loss.
- Start uses one canonical indexed StartModel; Power/session isolation explicit; stale icons never persist; submenu sizes from content.
- Status popups anchor from source node with End alignment and intrinsic sizing; network shows real AP rows; calendar dims adjacent days via foreground override.
- Sticky notes scoped per workspace (signal-driven), edit/move/resize/color/bold/delete, v1 loads / v2 writes.
- Startup/performance: scoped profiler phases with shared interval helper; packaged-first icon resolve (desktop icon ~4s -> ~0.3ms); shell startup self-wall isolated from blocking Control round-trips.
- Window chrome converges on panel material #1b1e20 with 38x31 controls, centered 16px glyphs, 20px app icon, measured title.


## 0.0.4 — 2026-09-05

### Product identity

- Promoted FlameWM Rust WebRender workspace identity to 0.0.4.
- Added canonical `FLAMEWM_VERSION` and pinned workspace `calloop` to 0.14.4.

## 0.0.2 — 2026-09-05

### Architecture

- Removed the external `/engine` design completely.
- Removed `icewm-rust` as an active dependency/runtime base.
- Established `crates/flamewm-wm` as FlameWM's independent Rust/X11 WM.
- Made `.vendor/` strictly reference-only; active source/path dependency checks reject vendor/engine reach-through.

### Build fixes

- Fixed the 0.0.1 invalid Cargo metadata (`rust-version = "0.0.1"`).
- FlameWM-owned product crates use Rust 1.85 / edition 2024. Promoted renderer/shell crates preserve edition 2021 during parity convergence instead of mixing an edition migration into the port.
- Fixed false `libfontconfig.so.1` doctor failures by using loader-backed library discovery.
- Removed the dead-code-failing IceWM Rust build path instead of suppressing its warnings.

### FlameWM Render 0.0.6 convergence

- Source-promoted the 0.0.6 renderer implementation into active FlameWM WebRender crates.
- Added ordered pointer-motion coalescing without crossing release/event boundaries.
- Added release-optimized interactive build contract (`opt-level=3`, Thin LTO, one codegen unit).
- Disabled unused X11 graphics-exposure traffic on cached image copy operations.
- Added cached transparency masks and rounded border strokes.
- Added press/motion/release interaction phases required by real drag/selection semantics.
- Promoted the 0.0.6 V8 HTML/CSS/assets/controller behavior as the visual baseline.
- Added `./verify-visual` against the approved 1350x641 Xephyr reference.

### Methodology

- Integrated the SourcePort post-mortem as a permanent parity-development rule.
- For visual/prototype parity: exact source owner -> mechanical translation/copy -> wire -> compile -> differential -> refactor.
- IceWM remains useful engineering reference material, but no longer defines FlameWM's runtime architecture or source ownership.
