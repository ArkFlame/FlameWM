# FlameWM Pass 49 — Bounded X11 window-transition upload staging

**Baseline:** Pass 48 integrated source overlay; FlameWM workspace `0.1.0`. **Scope:** `flamewm-render-x11/src/xcomposite.rs` transition-capture upload, not wallpapers. This is an incremental source overlay, not a full standalone release (uploaded baseline lacks `assets/` and `.vendor/`).

## Source-grounded memory finding

`upload_pixmap_depth(display, capture, depth)` is shared by the Magic Lamp warp (24-bit pixmap) and ARGB transition overlays (32-bit pixmap). It accepted an already-owned `CapturedImage.argb` (four bytes per pixel), allocated a **second full-frame C buffer** via `malloc(width * height * 4)`, copied the entire capture, and passed the duplicate to `XCreateImage`/`XPutImage`. The duplicate remained allocated until after upload, even though the original capture is already retained for animation frames. The previous 3840×2160 staging buffer was **33,177,600 bytes = 31.64 MiB** per upload.

## Minimal repair

- Reuse one `malloc`-allocated XImage backing buffer capped at **4 MiB for normal row widths**, copy successive source rows into it, and call `XPutImage` at their correct pixmap Y offset. A final shorter strip uploads only the remaining rows. `XDestroyImage` continues to own and free that C buffer exactly once. Pixmap and GC failure cleanup paths are unchanged; the source capture remains unchanged and reusable.
- At 3840×2160, strips of 273 rows require eight `XPutImage` calls; staging is **4,193,280 bytes ≈ 4.00 MiB**, avoiding **28,984,320 bytes ≈ 27.64 MiB** of *peak staging allocation* relative to the full-frame 31.64 MiB duplicate. Original capture memory and X-server pixmap storage remain necessary. Exception: a pathological one-row capture wider than 4 MiB necessarily allocates that one full row; `MAX_CAPTURE_PIXELS` still bounds it.
- Added a Rust regression for strip sizing, last partial row and byte-order preservation of source slices, plus three source-level guards in `tools/test_flamewm_pass49_xcomposite_upload_staging.py`.

## Safety and scope

Rendering geometry, source alpha/pixel order, source capture retention, cache limits, window ownership, wallpaper, desktop layout, screenshot flows, SDDM and settings are untouched. This path is not hot-loop repaint: the capture is uploaded when initiating an animation. Multiple `XPutImage` calls may modestly increase CPU/X11 overhead and must be compared natively before claiming a performance improvement. Xlib XPutImage consumes source bytes during each call; the staging area is overwritten only after the call returns. One original XImage upload call is replaced by several same-format calls with updated destination offsets.

## Evidence and limitations

- 82/82 existing focused Python source, package and regression tests passed alongside new checks.
- Pass 49 source regressions **failed against Pass 48** (expected red) and **passed after the repair**.
- Archive integrity and independent extraction to be verified before final publication.
- **NOT RUN:** native Cargo/rustc, isolated X11 comparison of full-screen/partial-window transitions, and RSS/PSS measurements: toolchain and display are unavailable in the authoring environment. This optimization changes native XPutImage request count, so do not treat Python passes as runtime or visual certification.

## Native acceptance on the full checkout

```bash
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-x11 --lib xcomposite::tests
CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-x11 -p flamewm-wm-x11
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Compare Magic Lamp and fade transitions in a nested X11 session at 1080p and 4K, checking correct pixel position/opacity and every edge/partial strip, and benchmark upload latency plus resident memory against Pass 48 under the same server and hardware. Do not replace the user's live display, change SDDM, or clear user configuration for a test.

**Skipped:** arbitrary cache-size reductions, wallpaper optimizations, invasive renderer or lifecycle refactors. **Risk:** additional XPutImage requests and unverified Xlib strip behavior; native smoke testing required.
