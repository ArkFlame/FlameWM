# FlameWM 0.1.0 Pass 42 — Native Screenshot integration

## Scope and source authority

Integration baseline: the user-supplied Pass 40 Codex continuation source overlay, with the Pass 41 desktop compiler hotfix overlaid. Added the supplied `FlameWM_0.1.0_Native_Screenshot_Implementation.tar(1).gz` feature patch. This delivery is a **merged source overlay**, not the user's complete asset-bearing Git checkout, `.deb` binary, or proof of a successful native X11 screenshot.

The incoming feature patch has 21 members. Twenty were applied; its `MANIFEST.sha256` was *not* copied because it is tied to a different source tree and references `.git` and files absent in the delivered snapshot. The previous historical checksum manifest was retained (it is not a valid manifest for this overlay). Archive identity must be established by the delivered tarball's own SHA-256.

## Integrated functionality

- New on-demand Rust/X11 binary: `crates/flamewm-screenshot/`, with root/current-monitor/active-window/pointer-window/region capture; PNG save, optional native annotation and clipboard, optional ffmpeg video, and explicit cancellation.
- New shortcut action IDs and default bindings: `Print` (all screens + clipboard), `Shift+Print` (region + annotation + clipboard), `Ctrl+Print` (active window + clipboard); routed from the WM and editable through Settings.
- New `flamewm-screenshot.desktop` entry and action shortcuts for Start/application discovery.
- Integrated the crate into Cargo.lock and workspace auto-membership; Debian optional ffmpeg recommendation; binary and launcher installed by both package installation paths.
- Preserved Pass 40 multi-monitor, session, wallpaper, launcher, Settings, and lock work and Pass 41 desktop compilation corrections.

## Source checks performed here

- `python3 -m unittest discover -s tools -p 'test_flamewm_screenshot_integration.py' -q`: 8 passed.
- `python3 -m unittest discover -s tools -p 'test_flamewm_package_pipeline.py' -q`: 7 passed; uses fixture binaries, does not build Rust.
- `python3 -m unittest discover -s tools -p 'test_package_source_archive.py' -q`: 6 passed.
- `python3 -m unittest discover -s tools -p 'test_flamewm_pass3*.py' -q`: 19 passed.
- A broad, unbounded `test_*.py` discovery was stopped at timeout; it includes specialized graphical/performance acceptance entry points and **does not establish a pass/fail of all tests**.

## Required native acceptance on the complete user checkout

1. Apply the new overlay with `tar --touch -xzf` to force Cargo to observe current source mtimes. Do not apply Pass 40/41 afterward.
2. `CARGO_BUILD_JOBS=2 cargo check -p flamewm-screenshot --all-targets`
3. `CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot`
4. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`
5. `CARGO_BUILD_JOBS=2 cargo test -p flamewm-platform -p flamewm-settings -p flamewm-wm-x11`
6. `CARGO_BUILD_JOBS=2 ./scripts/build-deb`. Require `FLAMEWM-DEB-READY` and check `SHA256SUMS` before installing.
7. On an isolated X11/Xephyr display, check all three screenshot shortcuts, save/copy, region Esc cancellation, PNG visual colors, annotation, clipboard INCR, and multiple unequal monitors. Check no stuck input grabs. Verify optional ffmpeg recording independently.

The sandbox used for this merge has **no `cargo` or `rustc`** and lacks the full FlameWM `assets/` and `.vendor/` trees. Consequently neither Rust compilation nor graphical screenshot capture, physical multi-monitor behavior, and Debian packaging of real binaries can be claimed. The package *fixture* passes, which is weaker evidence.

## Known feature limits (inherited from supplied screenshot implementation)

X11 only; captures visible window pixels. No Wayland portal, screenshot OCR, typed-text annotations, system-audio video recording, or Spectacle DBus compatibility. The clipboard owner remains resident only if image copying is requested. Do not claim Spectacle parity.

**Integration status:** source merged and focused source/package checks passed; native build and hardware functionality unverified.
