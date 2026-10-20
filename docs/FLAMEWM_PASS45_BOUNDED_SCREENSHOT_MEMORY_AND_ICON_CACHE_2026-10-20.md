# FlameWM 0.1.0 — Pass 45: bounded screenshot memory and icon-cache enforcement

**Baseline:** Pass 44 integrated screenshot / Pass 43 shell visibility / Pass 41 desktop compile repairs. **Deliverable:** a replacement **source overlay** for application onto the user's complete FlameWM checkout. It includes prior available source changes, not the missing `assets/` or `.vendor/` trees. Do not describe this overlay as a self-contained full-source or binary release.

## Source-led audit and decisions

| Owner | Finding | Decision |
| --- | --- | --- |
| `flamewm-screenshot/src/ui.rs` | Native X11 pixel conversion allocated one entire second framebuffer (`row_bytes * image.height`) before sending it in X11-sized chunks. | **Fixed:** reuse a native-format row batch, at most ~64 KiB with normal 32-bpp display widths, or one scanline if the scanline is larger; always respect the server's smaller request limit. Format handling retains 16/24/32-bpp, endianness, mask conversion and row padding. Added direct packing tests for padding, row splitting, RGB565 and both byte orders. Imported `RequestConnection` explicitly for the `maximum_request_bytes` trait, avoiding the same missing-trait failure previously found in screenshot clipboard. |
| `flamewm-screenshot/src/capture.rs` | Region selection already owns the fullscreen capture, but `Image::crop(&self)` allocated another pixel buffer and copied every selected scanline. | **Fixed:** consuming in-place crop (`crop(mut self, ...)`) with overlap-safe `Vec::copy_within`, fast identity return, explicit buffer shape checking, truncation and conditional shrink for small selections. Public usage is a single consuming call in screenshot main; existing crop and PNG tests remain. |
| `flamewm-integrations-linux/src/icons.rs` | `RasterCache::insert` could admit an item *larger* than its byte budget after evicting everything, leaving a resident out-of-budget raster and unnecessarily destroying the working set. | **Fixed:** do not retain over-budget entries. The requesting caller still receives its raster; smaller cached icons survive. A Rust regression asserts preserved entries and `used_bytes <= budget_bytes`. This applies to 8-MiB default and 512-KiB worker caches. |
| `flamewm-shell/src/async_projection.rs` | Decoded icon raster cache capped at 8 MiB; cache hits clone `RuntimeImage` to deliver document-owned pixels. | **Not changed.** Ref-counting requires changing ownership at canonical UI document/render boundaries and native tests; changing budget alone might raise file/cache misses. |
| `flamewm-desktop/src/wallpaper_loader.rs` | Bounded one-worker/latest-request mailbox; per-output composition uses a temporary monitor-sized raster, and retains the final root-sized frame. | **Not changed.** Avoiding this intermediate requires pixel-identical direct-into-region composition and is riskier without native visual tests. |
| `flamewm-image-core/src/blur.rs` and retained X11 surfaces | Blur scratch is reusable/reclaimable; X11 render surfaces use retained buffers for flicker prevention. | **Not changed.** Blind elimination risks recurring flicker and repaint overhead. |
| `tools/flamewm_desktop_services.py` | Long-lived Python companion is a possible baseline PSS contributor. | **Not changed.** Replace only with measured process PSS and proven native provider parity for autostart, notifications/polkit, and session lifetime. |

## Allocation math, not measured RSS

For a **3840×2160, 32-bpp** screenshot/editor: previous full native staging allocation = `3840 × 2160 × 4 = 33,177,600 bytes` (~31.64 MiB). The new buffer is **4 rows × 15,360 bytes = 61,440 bytes** (~60 KiB), assuming the X server accepts at least 64 KiB requests. This removes ~31.6 MiB *from that one temporary allocation* during screenshot editing, **not** from measured overall process PSS/RSS; PNG encoding, original capture, X11 server pixmaps and protocol buffers are still required. `Image::crop` avoids allocating a second full-sized pixel buffer for a large region, and conditionally releases oversized capacity after small crops. Neither changes normal desktop idle memory.

The raster-cache change is a genuine resident-byte-budget correctness fix but has no measured MiB savings: it affects oversized cache insertions only. Cache misses on repeated oversized requests are the deliberate trade-off.

## Verification and remaining gates

- Original Pass 44 source **fails** three of the five new Python source contracts (full-frame staging, copying crop, oversized icon admission); Pass 45 passes all five. These guards verify implementation shape, not native behavior.
- Focused legacy Python source/packaging test suites pass. Rust unit tests added in `ui.rs`, `capture.rs`, `icons.rs` **have not run here**: no `cargo`, `rustc`, X server, or full assets in this container.
- Preserve `flamewm-shell` Pass 43 public API fixes, Pass 41 desktop ownership corrections, screenshot shortcuts and Debian registration, panel geometry, monitor identity, and settings persistence.
- The previous source-only architecture warning set and physical 2-monitor, lockscreen, SDDM and KDE/KWallet behavior remain unverified. Do not mistake a passing source-contract test for runtime certification.

## Apply to the full checkout and verify

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS45_BOUNDED_SCREENSHOT_AND_ICON_CACHE_MEMORY_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
tar --touch -xzf "$ARCHIVE" -C . && \
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot -p flamewm-integrations-linux && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-shell -p flamewm-desktop && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

If all checks pass, `./scripts/xephyr` can be run in an isolated test display to exercise screenshot capture/annotation and multi-monitor behavior. For process memory measurements, FlameWM already supplies `./scripts/bench-memory`; compare matching workloads and screen geometry before and after, especially a large screenshot annotation and multiple app icons. **Do not apply older Pass 41–44 overlays afterward**, since that would reintroduce older source.

**Skipped:** native Cargo compilation/test execution, graphical screenshot verification, actual PSS/RSS measurements, and distribution of absent binary/assets/vendor trees. **Risk:** although changes are localized and designed to preserve pixels/protocol semantics, full Rust compilation and X11 integration checks remain mandatory.
