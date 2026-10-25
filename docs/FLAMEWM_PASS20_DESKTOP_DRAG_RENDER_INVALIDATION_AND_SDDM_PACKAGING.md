# FlameWM Pass 20 — Desktop drag redraw and Debian/SDDM handoff

## Baseline and exact failure

Pass 19 exported source. Desktop drag ghost is initialized by `flamewm-desktop/src/main.rs::show_drag_ghost` and moved through `UiDocumentAccess::position("desktop-drag-ghost", ...)`. On threshold crossing the ghost is shown and source icon is hidden; subsequent motion updates can alter the override without causing a retained redraw. The visible report: nothing follows the mouse (or the ghost appears only for a frame).

Canonical owner: `crates/flamewm-render-core/src/model/geometry.rs`. `RuntimeDocument::set_position_px` and `set_size_px` previously mutated `geometry_overrides` without calling `touch()`. The X11 renderer `redraw_if_dirty` explicitly skips when both damage is empty and the retained layout revision matches. Hence new pointer coordinates were not reliably painted. This also affects non-drag runtime repositioning.

## Implementation

- `set_position_px` compares new values with the current override; if changed, updates it and calls `self.touch()` to bump scene revision and mark Full damage. Unchanged coordinates remain no-ops.
- `set_size_px` uses the same invariant, including unchanged-size no-op. This prevents excessive scene repaint caused by the ghost's repeated 82×82 size projection every mouse motion.
- Rust model tests prove first and subsequent pointer moves change both layout coordinates and scene revision/damage, and unchanged moves do not repaint. The size test covers the same geometry contract and invalid input.
- Offline source-contract tests verify the entire drag projection and renderer invalidation chain and the pre-existing Debian session registration.
- No workspace, context menu, WM or other user preferences were changed.

## Native acceptance (must run on the user's full repository)

1. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`.
2. `RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-core -p flamewm-desktop -p flamewm-render-x11`.
3. `./xephyr`. Press a desktop item, move beyond the drag threshold, and cross multiple empty cells. Confirm the 50%-opaque icon **and text** continuously follow the cursor, without a fixed ghost left in the old cell. Release and confirm one final icon in its new position, no ghost; repeat with multi-select and cancellation. Verify normal click selection, context menu and Convert remain intact.
4. Verify renderer responsiveness with quick movements (including pointer motion coalescing) and no spurious repeated redraws when x/y are unchanged. Check both X11 composited and noncomposited fallback where possible.

## Debian package and SDDM session

This source archive is an integration source export, NOT a `.deb`: it lacks the full `assets/` tree and cannot be used alone to build the release. Apply the source changes to the **complete** local repository. On an x86_64 Ubuntu/Debian build system with Cargo and dependencies:

```bash
CARGO_BUILD_JOBS=2 cargo build --release --workspace
./scripts/package
python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.0_amd64.deb
```

Package path: `.target/dist/flamewm_0.1.0_amd64.deb`.

Install on the *other* x86_64 computer by copying the generated file and running:

```bash
sudo apt install ./flamewm_0.1.0_amd64.deb
```

`./scripts/package` already stages `packaging/flamewm-session.desktop` to `/usr/share/xsessions/flamewm.desktop`, with `Exec=/usr/bin/flamewm-session` and `TryExec=/usr/bin/flamewm-session`. SDDM automatically discovers it. No manual symlink or post-install hook is necessary. Select **FlameWM** in SDDM's login session picker; ensure Xorg/X11-capable SDDM exists. The `.deb` does not install SDDM, and no Wayland session is claimed at this pass. Superseded: the package now also installs a preliminary `FlameWM (Wayland)` entry; see `docs/WAYLAND_PLAN.md`.

## Verification limits

The authoring environment has no Cargo/rustc and the source export has no full assets. Rust compile/tests, `.deb` creation, native X11 behavior, and installation on SDDM cannot be certified here. Offline tests plus reproducible patch/archive checks are the available evidence; do not mark the product fully verified until native gates pass.
