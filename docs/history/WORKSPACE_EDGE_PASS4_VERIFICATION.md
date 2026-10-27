# FlameWM workspace edge-drag — fourth source pass (2026-10-18)

This source archive continues Pass 3; it is **not** a compiled or native-verified release. Apply individual changes onto the newer live Git checkout after reviewing dirty paths; never overwrite user work wholesale.

## Changes

1. `crates/flamewm-wm-x11/src/wm/pointer.rs`: when the edge-dwell deadline fires, re-query the actual X11 pointer and require the `KeyButMask::BUTTON1` flag before moving a workspace. Cancel a stale candidate if the physical button was released before the queued ButtonRelease event was handled. Passive EnterNotify on another window cannot steal focus during an active pointer session, including following edge-pointer warp.
2. `crates/flamewm-wm-x11/src/wm/workspace.rs`: resize the per-workspace `workspace_focus` table whenever workspace count changes, and preserve the active window's focus slot where it remains visible. Previously a newly added workspace had no focus-restoration slot.
3. `crates/flamewm-shell/src/async_projection.rs`: extend the existing test to prove workspace-only taskbar projection actually swaps the visible task's label, not just the pager, when one of the windows is minimized and both are on the same output.
4. `tools/flamewm_workspace_edge_acceptance.py`: prior to dragging, confirm mere edge hover doesn't switch; after every held crossing, verify the carried window stays mapped, other workspaces' windows are hidden, the pointer remains in the dragged titlebar, and at least half the available frame is visible. Includes left-edge wrap assertions.
5. `tools/test_flamewm_workspace_edge_acceptance.py`: tests for frame/titlebar grip and on-screen arrival checks.

## Verification matrix

The only code executing in this authoring container is Python; it has Xvfb but no Rust compiler/Cargo, no live WM binary, and no external network access to install the toolchain. **No Rust compilation or native FlameWM X11 PASS is claimed.**

- Executed authoring-environment gate outcomes are listed in the accompanying assistant response; new Python probe assertions do not substitute for an actual WM run.
- `MANIFEST.sha256` is inherited from the trimmed source export and is not a regenerated manifest for this modified archive.
- The architecture/provenance guard failures from omitted `.gitignore` and vendor/promoted RustWebRender source are known inherited limitations; rerun in the real checkout.

## Mandatory live verification (one Cargo owner, private display)

```sh
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-wm-x11 -p flamewm-shell -p flamewm-shell-core
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test --workspace
python3 tools/architecture_guard.py
python3 tools/ui_contract_guard.py
python3 tools/verify_rwr_promotion.py
CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-wm
python3 tools/flamewm_workspace_edge_acceptance.py --wm-bin "$(pwd)/.target/release/flamewm" --work-dir /tmp/flamewm-workspace-edge-pass4
```

The native check must show 0→1→2→0 while the same button remains held, 0→2 on left-edge wrap, opposite-edge pointer warps at constant Y, continued window movement after transfer, and no taskbar entries from a different workspace. The provided native harness tests the WM and EWMH; taskbar *visual* filtering requires a full WM + shell native session. Also test dynamic workspace count changes, missing primary-button release, focus-follows-mouse, multiple displays and minimized/sticky windows. Report unavailable checks as NOT_RUN and inspect `wm.log` and `xvfb.log` on failure.
