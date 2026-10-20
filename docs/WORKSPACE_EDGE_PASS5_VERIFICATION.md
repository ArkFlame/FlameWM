# FlameWM 0.1.0 — workspace and edge-drag pass 5

Status: **SOURCE PATCH — REQUIRES RUST BUILD AND NATIVE X11 VERIFICATION** (2026-10-18). This archive extends Pass 4; it is not evidence of a working binary. The trimmed source export omits governance/promotion files that its guards expect. Merge onto the actual live Git checkout without overwriting dirty files.

## Scope of this pass

1. `crates/flamewm-wm-x11/src/wm/workspace.rs`: `apply_workspace_visibility` no longer allocates an IDs vector plus an old/new visibility-pairs vector for every workspace switch. It directly iterates the existing managed-window map with separate borrows of `self.clients` and `self.conn`, and returns one `bool` indicating whether any frame visibility flipped. The old `mark_windows` condition is maintained. This is an **allocation-elimination claim from source structure, not a measured CPU/RSS improvement**.
2. `crates/flamewm-wm-x11/src/wm/mod.rs` and `wm/tests.rs`: `frame_visibility_delta` is the pure canonical check shared by workspace switching and an explicit `_NET_WM_DESKTOP` window transfer. Existing tests for workspace-change invalidation now assert boolean cases. New regression tests cover visible→hidden, hidden→visible, hidden→hidden (must be no-op), and visible→visible (no-op).
3. `crates/flamewm-wm-x11/src/wm/workspace.rs`: moving a minimized or already-hidden client between inactive workspaces no longer sends redundant `XUnmapWindow` requests or increments `ignore_unmap`. Moving a focused client out of the active workspace clears the stale EWMH active-window state and corresponding focus-restoration entry.
4. `crates/flamewm-wm-x11/src/wm/pointer.rs`: a stationary edge-dwell deadline rejects QueryPointer results belonging to another X screen (`same_screen=false`) before testing button mask/coordinates.
5. `tools/flamewm_workspace_edge_acceptance.py`: expanded the private-Xvfb fixture to focus the second window, move it to an inactive workspace and verify `_NET_ACTIVE_WINDOW` stops naming it; then repeatedly transfer it between *two inactive* workspaces without letting it become visible. Original held-drag, pointer-warp, wraparound and mapping checks remain.

## Verification in the authoring container

- `python3 -I tools/test_flamewm_workspace_edge_acceptance.py`: 6 tests PASS.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_xtest_*.py' -q`: 53 tests PASS.
- `python3 -I tools/test_architecture_guard_loc.py`: 2 tests PASS.
- `python3 -I tools/ui_contract_guard.py`: 0 errors, 25 existing warnings.
- `python3 -I -m py_compile tools/flamewm_workspace_edge_acceptance.py`: PASS.
- Native acceptance with a deliberately absent WM binary exits **3 / BLOCKED**, not a false PASS.
- `python3 -I tools/architecture_guard.py`: exits 1 because this archive lacks `.gitignore`; the untouched Pass 4 archive fails identically.
- `python3 -I tools/verify_rwr_promotion.py`: exits 1 because this archive lacks historical `rustwebrender-x11` promotion owners and expected motion anchor; untouched Pass 4 fails identically.
- **Cargo, rustc and rustfmt are not installed** in this authoring environment; Rust compilation, Rust tests and live X11 WM behavior **NOT_RUN**. There is no internet access to obtain the toolchain. Do not interpret source-only tests as a runtime acceptance result.
- `MANIFEST.sha256` is inherited from an earlier, more complete source export, not refreshed; many of its listed `.vendor`/`.agents` paths are deliberately absent from the uploaded archive. It is not a valid manifest of this delivered artifact.

## Mandatory live-checkout commands

Run on your Linux system **after copying the patch onto live source and preserving uncommitted changes**. Use one Cargo owner and an isolated X server; do not run `pkill` against names or touch the login display.

```bash
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-wm-x11 -p flamewm-shell -p flamewm-shell-core
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test --workspace
python3 tools/architecture_guard.py
python3 tools/ui_contract_guard.py
python3 tools/verify_rwr_promotion.py
CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-wm
python3 tools/flamewm_workspace_edge_acceptance.py --wm-bin "$(pwd)/.target/release/flamewm" --work-dir /tmp/flamewm-workspace-edge-pass5
```

Full acceptance requires a live shell as well as a WM: switch workspaces while normal, minimized, pinned and sticky windows are present, and check that taskbar tasks reflect **only the active workspace**, while pinned launchers remain and sticky windows display where appropriate. For edge dragging, verify 0→1→2→0 and 0→2 wrap without releasing primary button; stable Y, opposite-side pointer warp, continuous grip, release stops switching, and windows in inactive workspaces remain unmapped. Exercise one and multiple monitors, resize/maximized cases, cancel/unmanage, and creation/removal of workspaces. Report any unavailable scenarios `NOT_RUN`.

## Compatibility note

The new X11 operations remain scoped to WM code. This pass does not modify UI styling, rendering, skins, user settings or the current Magic Lamp behavior; these are protected. Do not declare completion from source inspection or unit tests alone.
