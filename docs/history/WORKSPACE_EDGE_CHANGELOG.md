# FlameWM workspace isolation and edge-drag transfer (2026-10-18)

## Changed owners
- `crates/flamewm-shell-core/src/lib.rs`: one canonical workspace/output-aware task filter; sticky windows visible everywhere, pinned launchers retained, minimized window stays on its workspace.
- `crates/flamewm-shell/src/{runtime,projection,async_projection,app}.rs`: synchronous and asynchronous panel task projections, click mapping, icon geometry and icon prewarming now respect current workspace; workspace snapshot signals invalidate task slots as well as pager.
- `crates/flamewm-wm-x11/src/wm/{mod,pointer,workspace,tests}.rs`: while an *activated titlebar move* is held, a 220 ms dwell within 4 px of the outer root left/right edge selects previous/next workspace with wrap. WM reassigns the moved client, switches visibility, preserves move controller and pointer grab, rebases the frame and warps the pointer to the opposite edge at the same Y coordinate.

## Validation status
- Source-level review and archive-integrity checks performed in the authoring environment.
- **NOT RUN:** `cargo check`, `cargo test`, Xephyr/XTEST or a real display: this environment contains no Rust toolchain and has no internet access to install one. Do not treat this archive as a verified release.

## Mandatory checks on the actual checkout
1. Compare this snapshot with live HEAD and preserve existing dirty files; apply only the nine touched paths if upstream diverged.
2. `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-shell-core -p flamewm-shell -p flamewm-wm-x11`.
3. `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo check --workspace --all-targets`; then `cargo test --workspace`, `python3 tools/architecture_guard.py`, `python3 tools/ui_contract_guard.py`, and `python3 tools/verify_rwr_promotion.py` if these gates exist in live checkout.
4. Owned Xephyr: create at least 3 workspaces with distinct normal and minimized windows. Switch using keyboard and pager; assert task slots, click targets, icon geometries and sticky/pinned exceptions.
5. With 3 workspaces, drag window by its titlebar to either *outer screen edge*, dwell ~220 ms, check wraparound, window `_NET_WM_DESKTOP`, `_NET_CURRENT_DESKTOP`, pointer X/Y, continuous grab, and uninterrupted move. Then release and verify no snap / extra wrapping. Resize, ordinary pointer hover, and one-workspace sessions must never transfer.
6. Repeat at 1×/1.5× scale and on multioutput geometries including a negative origin. Ensure popup, Magic Lamp, snap, keyboard workspace switching and ordinary theme behavior are unchanged.

## Design tradeoffs / known limitations
- Edge dwell requires subsequent pointer motion (there is no extra timer or polling); holding perfectly still after first entering the edge does not trigger until another motion event.
- Edge navigation uses the *outer X11 root screen* boundary, not internal monitor seams; this is deliberate to avoid accidental transfer between physical outputs.
- Pinned apps remain launchers in other workspaces instead of focusing a hidden instance. Sticky apps appear everywhere. Task counts and window state remain authored by the WM.
- Exact native pointer-grab continuity, X11 server behavior, and race handling are not proven without an owned X11 display. Do not report those acceptance gates as PASS until witnessed.

## Second pass — held-edge deadline and arrival placement

Further changes in `wm/mod.rs`, `wm/pointer.rs`, `wm/workspace.rs`, and `wm/tests.rs`:

1. **No extra pointer motion needed.** The existing reactor's blocking timeout is bounded by the remaining 220 ms edge dwell. On the deadline, the WM reads the actual root pointer once and verifies the same window is still in an activated titlebar move at the same edge. No periodic polling or extra thread is introduced.
2. **Onscreen arrival grip.** The transfer repositions the floating frame at the destination side and rebases the controller to the new cursor, instead of preserving a potentially huge stale pointer/frame offset that can leave almost all of a wide window offscreen. Next motion continues from the new placement.
3. **Monotonic timing.** The dwell uses `Instant`, independent of wrapping X11 event timestamps and stale queued motion timestamps. Switching sides or moving away cancels/restarts the candidate. Releases clear it.
4. **Pure regression contracts.** Unit tests cover 219/220 ms, wait capping versus another reactor deadline, no-pending blocking, left/right wrap, negative screen origin, narrow/wide window arrival grips.

### Verification performed in the authoring environment

- **PASS**: 638 environment-independent Python `tools/test_flamewm_*.py` tests, 0 failures (excluded the D-Bus fake-portal test module, which requires unavailable `dbus` Python bindings and a usable session bus).
- **PASS**: `python3 tools/ui_contract_guard.py`: 0 errors, 25 warnings (historical classes and prototypes).
- **PASS**: `python3 -I -m unittest discover -s tools -p test_architecture_guard_loc.py -q`: 2 tests.
- **BASELINE RED, not introduced**: `python3 tools/architecture_guard.py` requires the `.gitignore` not shipped in this trimmed source; the untouched `116_18_10_26.tar.gz` fails identically.
- **BASELINE RED, not introduced**: `python3 tools/verify_rwr_promotion.py` requires the omitted RustWebRender vendor source and promotion anchors; the untouched `116_18_10_26.tar.gz` fails identically.
- **ENVIRONMENT BLOCKED**: no `cargo`, `rustc`, `rustfmt`, network resolution or compiled FlameWM binaries in the tool environment. No Rust compile, Rust unit, Xephyr or installed-desktop runtime verdict can be claimed. Xvfb alone cannot execute an unbuilt WM.
- **PENDING**: the original archive's `MANIFEST.sha256` contains paths excluded from this source export and is not a self-consistent post-edit manifest. It is retained unchanged as historical data, not advertised as this tarball's checksum proof.

### Minimum clean-checkout native gate

```sh
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-shell-core -p flamewm-shell -p flamewm-wm-x11
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test --workspace
python3 tools/architecture_guard.py
python3 tools/ui_contract_guard.py
python3 tools/verify_rwr_promotion.py
```

Run on a private X11 display with a fresh binary and 3 workspaces. Drag to the physical left/right edge, hold the pointer stationary for 220 ms, assert `_NET_CURRENT_DESKTOP` and `_NET_WM_DESKTOP`, and check that the window stays attached to the cursor and mostly onscreen after the warp. Include negative-origin/multi-output setups where available, check focus/keyboard navigation, drag-release/no-snap, non-drag cursor movement, and minimize/restore. Any failing real gate requires a reproduction-first repair and fresh verification, not a declared PASS based on these static findings.

## Third pass — safe X11 server grab and native acceptance fixture

### Correctness repair (WM)

`wm/workspace.rs` previously entered `XGrabServer`, then invoked the *full*
`reduce_and_execute` path before `XUngrabServer`. That path can run
`FrameEffect::PaintChrome`, whose `paint_chrome` synchronizes the WM X connection
and uses the renderer's **independent** Xlib connection. That can stall the
renderer while another client is excluded by the server grab.

The transfer now plans geometry using `interaction::step_owned` before grabbing
the server. One narrow `defer_drag_arrival_paint` contract in `wm/mod.rs` allows
only position/no-op and client-root notification effects in the atomic section.
The canonical `execute_effects` applies these effects; the current workspace
EWMH property, client membership, visibility changes, native move and pointer
warp are submitted within the same server grab. `XUngrabServer` is flushed
**before** deferred `paint_chrome` touches the renderer connection. Window
placement and the retained move session are still owned by the existing reducer
and session, not duplicated in a new gesture framework.

New Rust tests in `wm/tests.rs` drive the *real pure move reducer* to verify the
arrival placement and ensure renderer effects are deferred; unsafe effects are
rejected rather than executed while the server is grabbed. These tests are
**added but not compiled/executed** in this environment.

### Dedicated owned-display verification

New executable-source tool:

```sh
# After building fresh binaries on the real checkout:
python3 tools/flamewm_workspace_edge_acceptance.py \
  --wm-bin "$(pwd)/.target/release/flamewm" \
  --work-dir /tmp/flamewm-edge-test-001
```

The harness allocates a free private Xvfb display in `:97..:119`, isolates
HOME/XDG paths, starts only its own WM, creates two real X11 client windows,
confirms per-client `_NET_WM_DESKTOP`, drives a titlebar drag via XTEST, and
checks consecutive right-edge transfers (0→1→2→0), left-edge wrap (0→2),
opposite-side pointer warp at unchanged Y, continued drag without releasing,
then visibility and membership after release. It never calls `pkill` or touches
`DISPLAY=:0`. Exit 0=PASS, 1=behavior failure, 3=environment block. The
harness's offline unit tests ran **4 passed**; this is not evidence of a native
WM pass, which requires a real built binary.

### Executed authoring-environment checks

- `python3 -I tools/test_flamewm_xtest_drag.py`: **10 passed**, exit 0.
- `python3 -I tools/test_flamewm_workspace_edge_acceptance.py`: **4 passed**, exit 0.
- `python3 -I -m unittest discover -s tools -p test_architecture_guard_loc.py -q`: **2 passed**, exit 0.
- `python3 tools/ui_contract_guard.py`: **exit 0**, 0 errors / 25 warnings.
- `python3 -I -m py_compile` for the two new harness files: **exit 0**.
- Full independent Python sweep: **no completed verdict** (timed out).
- `python3 tools/architecture_guard.py`: **exit 1**, omitted `.gitignore` in
  export (same baseline limitation as the untouched source archive).
- `python3 tools/verify_rwr_promotion.py`: **exit 1**, omitted vendor/promoted
  RustWebRender sources from export (same baseline limitation).
- `cargo check`, `cargo test`, `rustfmt`, X11 WM integration: **NOT RUN**;
  rustc/cargo/compiled WM unavailable in the authoring container. Runtime proof
  and packaging/deployment acceptance must occur on the real Linux checkout.

**Do not apply an entire older snapshot over newer live Git without first
reconciling its source diff and preserving user WIP.** In particular, the latest
reported user WIP was `.debug/*`, `crates/flamewm-settings/src/theme_picker.rs`
and `crates/flamewm-shell/src/projection.rs`. No new changes to those files are
made in this third pass.
