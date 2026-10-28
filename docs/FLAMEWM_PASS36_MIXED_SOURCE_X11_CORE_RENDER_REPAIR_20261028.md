# FlameWM 0.1.3 — Pass 36: mixed source / graphics API and X11 depth safety

## Baseline and authority

This release starts from the **complete Pass 34 source archive**. It is the independent Rust workspace, not the historical IceWM C++ fork. The newly supplied `E0432` build log is the direct failure evidence. Source and archive contents were compared before editing.

## User-reported `E0432` root cause

Both `crates/flamewm-render-x11/src/app/mod.rs` and `src/external_drawable.rs` import `flamewm_api::graphics`. The **Pass 34 archive does contain** `crates/flamewm-api/src/graphics.rs`, and the corresponding `lib.rs` contains `pub mod graphics;`. Thus the reported unresolved import indicates that the *local build tree* contains mismatched source revisions, or an equivalent divergent API source. The previous archive was packaged under `flamewm/`; the extraction command `tar -xzf <archive> -C .` without `--strip-components=1` would create a **nested `flamewm/` directory** instead of replacing the repository files. The observed build error is consistent with that installation mistake, but the exact on-disk cause on the target PC was not remotely verified.

### Implemented fixes

- `scripts/package-source-archive` now produces mode `0644` output rather than inheriting `mkstemp`'s restrictive `0600`, and emits **root-relative** entries (`Cargo.toml`, `crates/...`, `scripts/...`) rather than a parent `flamewm/` directory. The straightforward `tar -xzf ... -C .` command will no longer create that nested source tree. Existing users should still back up dirty working files before overwriting a checkout.
- `tools/verify_cargo_manifests.py` now checks the renderer's referenced cross-crate `graphics` source/export contract in addition to the TOML layout. `./scripts/wayland` runs this before Cargo, and source archive generation runs it before publishing a package.
- A mutation-based regression test deliberately simulates an old API `lib.rs` and a missing `graphics.rs`, asserting that both are rejected. This is a guard, not a replacement for compiling the Rust workspace.

## Separate X11 graphics bug found during audit

X11 `XCopyArea` requires matching depths for source and destination. The previous non-XRender fallback created a 32-bit ARGB pixmap and attempted a core-X11 copy into the live drawable, which may use depth 24, causing `BadMatch`. It also accepted non-opaque pixels in a secondary blit route without checking that core X11 has no alpha blending.

- `ExternalDrawableTarget::blit_cached_argb32` now queries the actual drawable depth only on core-provider cache misses, allocates a matching pixmap, and uses a native-mask-correct opaque XImage upload. The XRender path retains the existing 32-bit ARGB premultiplied upload.
- `ExternalDrawableTarget::retarget` invalidates cached pixmaps when a drawable changes so one drawable's depth cannot contaminate another target.
- The other `rgba_blit` entry point now rejects translucent images **before** native pixmap allocation on core X11 and allocates a pixmap matching the backbuffer's depth. The code shares `upload_opaque_argb32_image` rather than adding a second RGB packing implementation.
- Removed the unnecessary `built = Some(words)` assignment reported in the build warning.

The code deliberately does not fake alpha blending where unavailable. The core path can render *opaque* images. Semi-transparent images require XRender; the method returns an explicit error otherwise.

## Source files affected

- `crates/flamewm-render-x11/src/app/images.rs` — native opaque upload using matching visual/depth; builds no extra runtime process.
- `crates/flamewm-render-x11/src/app/mod.rs` — exports shared upload helper.
- `crates/flamewm-render-x11/src/external_drawable.rs` — both paint consumers, actual X11 drawable depth and cache lifecycle.
- `tools/verify_cargo_manifests.py` — mixed-source preflight.
- `scripts/package-source-archive` — root-relative source paths.
- `tools/test_package_source_archive.py` and `tools/test_pass36_core_blit_source_consistency.py` — contract and deliberately broken-source regression cases.

## Verification run in this environment

- `python3 -I tools/verify_cargo_manifests.py` — **45 manifests, PASS**.
- `python3 -I tools/test_pass33_dependency_chrome.py -q` — **6 PASS**.
- `python3 -I tools/test_package_source_archive.py -q` — **20 PASS**.
- `python3 -I tools/test_pass34_graphics_and_manifest.py -q` — **5 PASS**.
- `python3 -I tools/test_pass36_core_blit_source_consistency.py -q` — **5 PASS**.
- `python3 -I tools/test_flamewm_wayland_session_matrix.py -q` — **14 PASS**.
- `python3 -I tools/test_pass29_wayland_visual_contract.py -q` — **7 PASS**.
- `python3 -I tools/test_pass28_review_cpak.py -q` — **6 PASS**.
- `bash -n scripts/wayland scripts/flamewm-wayland-session scripts/apply-pass30-cargo-layout` — **PASS**.
- `python3 -I scripts/package-source-archive --check . packaging/source-manifest.txt` — **PASS**.

These are source/packaging checks, not live X11 drawing tests. Native Rust/Cargo and the GPU/X11/Wayland environment are unavailable in this container; **`cargo check`, link, X11 alpha visual tests, Wayland titlebar screenshot comparisons, hardware execution and CPU/PSS performance are not verified**.

## Required native verification

```bash
python3 -I tools/verify_cargo_manifests.py
cargo check --locked -p flamewm-api -p flamewm-render-x11 -p flamewm-wm-x11
cargo check --locked -p flamewm-wm-wayland
cargo check --locked -p flamewm-wm-wayland --features drm
cargo test --locked -p flamewm-api -p flamewm-render-x11 -p flamewm-wm-wayland
./scripts/wayland
```

## Applying the complete archive

The Pass 36 archive is **root-relative**. Extract into a fresh test directory (safest):

```bash
mkdir -p "$HOME/Downloads/flamewm-pass36"
tar -xzf "$HOME/Downloads/FlameWM_0.1.3_Pass36_Mixed_Source_X11_Core_Fallback_Complete_20261028.tar.gz" \
    -C "$HOME/Downloads/flamewm-pass36"
cd "$HOME/Downloads/flamewm-pass36"
python3 -I tools/verify_cargo_manifests.py
./scripts/wayland
```

To update an existing clean worktree directly, run `tar -xzf "$HOME/Downloads/FlameWM_0.1.3_Pass36_Mixed_Source_X11_Core_Fallback_Complete_20261028.tar.gz" -C .` **from its repository root** after preserving all local changes. **Do not use `--strip-components=1` with Pass 36.** It will strip the actual first directory (`crates`, `scripts`, etc.) and cause an invalid source layout.

## Original objectives — current measured status

Prior progress estimate: approximately **55%**. The new source/build and X11 fallback fixes are essential stability work but do not constitute a new X11 OpenGL renderer or verified graphics acceleration. Do **not** raise the original-objective progress bar just because another source archive or test suite succeeded.

Unfinished: dedicated X11 GPU paint provider, unified font-quality parity with Wayland, proof of actual hardware GL execution, and a measured runtime dependency reduction. PipeWire-Pulse compatibility remains the chosen audio interface (no new PipeWire native client dependency).
