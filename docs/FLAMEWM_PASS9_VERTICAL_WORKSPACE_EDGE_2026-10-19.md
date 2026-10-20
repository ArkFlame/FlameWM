# FlameWM Pass 9 — vertical workspace-edge dragging and native harness convergence

Baseline: Pass 8 archive, already integrating the DeepSeek two-row workspace grid, Pass 6 cursor cooldown and the Pass 7/8 context-menu/Convert changes. This Pass 9 does not overwrite later live-repository work; integrate the `.patch` on the current checkout with review.

## Changes

1. `crates/flamewm-wm-x11/src/wm/mod.rs`: `DragEdge` now includes Top/Bottom. Edge detection uses a root-space point, rejects locations outside its root, and resolves corner conflicts to the nearest edge (horizontal on tie). `drag_edge_target` retains row-local left/right wraparound; top/bottom changes to the same column of the opposite row, with no transition into nonexistent odd-count grid cells. Both axes use the existing 220ms dwell state and isolated 500ms cooldown timers.
2. `crates/flamewm-wm-x11/src/wm/pointer.rs`: all four sides use the one retained move session and existing native workspace-transfer transaction; the arrival pointer destination uses the current frame height so a top-edge crossing can keep at least half of a window onscreen, provided it fits the display. Nothing switches on idle edge hover or without an activated move. The deadline path rechecks the physical button as in Pass 8.
3. `crates/flamewm-wm-x11/src/wm/workspace.rs`: vertical transitions use the same reducer, server-grab and deferred renderer path as horizontal transfers; the Y delta is rebased instead of the X delta, preserving the x coordinate for top/bottom motion. No new gesture/input authority.
4. `crates/flamewm-wm-x11/src/wm/tests.rs`: horizontal regression call sites updated, with new vertical geometry, row mapping, negative-screen guards, independent cooldown, corner and titlebar-grip reducer tests.
5. `tools/flamewm_workspace_edge_acceptance.py`: its preexisting three-workspace acceptance fixture assumed flat horizontal wrap `0 → 1 → 2 → 0`, contradicting the DeepSeek two-row grid (three workspaces means top `0 1` and singleton bottom `2`). The fixture now configures six workspaces, checks row-local `0→1→2→0` and `0←2`, then tests `2↑→5`, `5↓→2`, physical cursor attachment, independent axis cooldown, 500ms same-axis cooldown, and final EWMH map/unmap visibility. It still owns a private Xvfb session and refuses to pass without an executable WM binary.
6. `tools/test_flamewm_workspace_edge_acceptance.py`: offline regression ensures the fixture stays aligned with the grid.

## Verification performed in the source-export environment

- `python3 -I tools/ui_contract_guard.py`: 0 errors / 24 warnings.
- `python3 -I -m unittest discover -s tools -p test_flamewm_context_menu_weather_acceptance.py -q`: 14 passed.
- `python3 -I -m unittest discover -s tools -p test_flamewm_conversion_provider.py -q`: 2 passed (real ImageMagick).
- `python3 -I -m unittest discover -s tools -p test_flamewm_workspace_edge_acceptance.py -q`: 7 passed (offline harness tests; NOT a native X11 result).
- `python3 -I -m unittest discover -s tools -p test_flamewm_deb_acceptance.py -q`: 26 passed.
- `python3 -I -m unittest discover -s tools -p test_ui_contract_guard.py -q`: 8 passed.
- `python3 -I -m unittest discover -s tools -p test_package_source_archive.py -q`: 4 passed.
- Total of the preceding distinct Python unittest suites: **61 passed** (0 failures).
- `python3 -I tools/flamewm_workspace_edge_acceptance.py --wm-bin /no/binary`: BLOCKED with exit 3 as intended; no runtime pass was fabricated.
- Architecture/provenance checks fail as in Pass 8 due to missing upstream RustWebRender reference/asset paths and the unrelated `ui/desktop/desktop.css` guard violation. The paths touched by this pass are disjoint from the guard failures.

## Non-completion and risks

**Rust compilation, the Rust tests above, the release build, and the live X11 acceptance scenario were NOT run**: this container has no `rustc`/`cargo` or compiled FlameWM WM binary, and the supplied export omits the full `assets/` tree. An Xvfb executable by itself cannot verify a missing WM binary.

The native test depends on python-xlib + XTEST and on a complete full-repository build. Vertical top-edge arrival deliberately warps the pointer farther from the bottom for tall windows to maintain titlebar contact and keep at least half the frame visible; this is a policy to evaluate with real pointer feel in Xephyr. For windows exceeding output dimensions, physical visibility constraints may make full-frame visibility impossible. The two-row grid has no vertical neighbour under the empty last cell in odd workspace counts.

A true Wayland compositor/session remains unimplemented. The existing supported X11 login-session packaging is preserved; do NOT install an unusable Wayland session entry.

## Verify on full checkout

```sh
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-wm-x11
RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test --workspace
CARGO_BUILD_JOBS=2 cargo build --release --workspace
python3 -I tools/flamewm_workspace_edge_acceptance.py --wm-bin .target/release/flamewm
```

Use the repository's actual target directory if different. Then check multi-monitor roots, all 4 edge dwells, cancellation on release, odd workspace counts, maximized/fullscreen behavior, top-edge window visibility, and failure-recovery under a private X11 display. Do not claim production readiness until these gates pass.
