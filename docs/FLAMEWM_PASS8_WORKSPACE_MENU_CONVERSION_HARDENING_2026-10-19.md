# FlameWM 0.1.0 — Pass 8: workspace-menu lifecycle, conversion reliability, grid-aligned mouse dragging

**Baseline:** uploaded `FlameWM_0.1.0_deepseek_merged_context_convert_x11_pass7_2026-10-19.tar.gz` (DeepSeek plus Pass 6 merge). This pass is intentionally a small integration repair, not a source-tree rewrite. Preserve the latest live repository's dirty state when applying.

## 1. Real defects fixed

### Desktop context menu resurrected by subsequent projection

`crates/flamewm-desktop/src/main.rs`: the raw outside-press callback previously hid menu surfaces and cleared pending delete, **but did not clear `DesktopState.menu`**. A later `sync_menu` could re-open the visually dismissed menu. Both left-click selection paths likewise hid the surface while retaining `state.menu`.

The canonical `close_menus_on_outside_press` now clears the semantic menu and delete intent before hiding the menu/confirmation surfaces; the desktop-background and file-icon primary-press paths clear `state.menu` as well. Workspace change continues to clear state via `refresh_workspace_projection`. Tests extend the existing `outside_press_tests` to assert the semantic menu becomes `None` after an outside click and remains present for an inside click (including a Convert wheel).

### Mouse edge-drag navigation mismatched the DeepSeek two-row workspace grid

`crates/flamewm-wm-x11/src/wm/mod.rs`: DeepSeek keyboard left/right already used the two-row `flamewm_api::workspace` topology, but mouse edge dragging wrapped across a **flat** workspace list. Example: desktop 2 → right could unexpectedly jump into the next row. Mouse dragging now wraps **within the current row**. Four desktops `1 2 / 3 4`: 2 → right lands at 1, 3 → left lands at 4. Six `1 2 3 / 4 5 6`: 3 → right lands at 1, 4 → left lands at 6. Five `1 2 3 / 4 5`: lower row wraps only between 4 and 5. The policy uses the canonical grid position/column count; keyboard navigation remains non-wrapping as requested in the prior DeepSeek task.

`crates/flamewm-wm-x11/src/wm/pointer.rs` does not arm an edge-dwell timer for a row with only one workspace (e.g. lower row of 3), which would otherwise cause a zero-timeout reactor wake loop. The 220 ms dwell, 24 px opposite-edge pointer inset and 500 ms horizontal cooldown are retained.

**Unimplemented:** Vertical mouse-edge dragging (`Up/Down`) remains an explicit next step. The vertical cooldown *state* exists, but there is no vertical WM transfer path in Pass 8. It is not declared working. Keyboard up/down and the two-row grid remain from DeepSeek.

### Animated image conversion left hidden files behind

`crates/flamewm-desktop-core/src/conversion.rs`: observed with real ImageMagick: converting a two-frame GIF to JPG previously emitted `.flamewm-convert-123-0-0.jpg` and `-1.jpg` while leaving the reserved expected `.jpg` empty. The code rejected the zero-byte placeholder and only deleted that placeholder, leaving invisible numbered image fragments.

The converter now selects `[0]` of multi-frame inputs for single-image target formats (`png`, `jpg`, `bmp`, `ico`, `avif`), while preserving multi-frame sources for the animation/multipage-capable formats (`gif`, `webp`, `tiff`). Source paths containing spaces are passed as single process arguments; output paths are passed as `OsString` with a format prefix (no lossy filename conversion). This policy is explicit: conversions to static formats intentionally use the first frame.

Conversion jobs now have a **90-second child-process deadline**, a **maximum of two concurrent ImageMagick conversions per desktop process**, and cleanup on process failure, timeout and publication failure. The executable chooser ignores non-executable `magick` files so they do not mask a valid `convert` install. Existing atomic no-overwrite output publication and worker-thread dispatch are preserved.

## 2. Evidence run here

- `python3 -I tools/ui_contract_guard.py` → 0 errors, 24 warnings (existing visual/selector warnings).
- `python3 -I -m unittest discover -s tools -p test_flamewm_deb_acceptance.py -q` → 26 passed.
- `... -p test_flamewm_context_menu_weather_acceptance.py -q` → 14 passed.
- `... -p test_flamewm_workspace_edge_acceptance.py -q` → 6 passed (Python harness tests, **not native X11 runtime**).
- `... -p test_ui_contract_guard.py -q` → 8 passed.
- `... -p test_package_source_archive.py -q` → 4 passed after preserving original executable modes during local extraction. The initial permission errors were caused by the local extraction procedure, not upstream source.
- **New** `... -p test_flamewm_conversion_provider.py -q` → 2 passed using real ImageMagick: an animated GIF converts into exactly one JPG/PNG/BMP output per target, without numbered leftovers; animated WebP remains one output with two frames.
- Direct before/after ImageMagick reproduction on a 2-frame GIF: baseline CLI without `[0]` produced two numbered JPEGs and a zero-byte expected output; the corrected `[0]` invocation produced one valid JPEG. Direct PNG and animated WebP probes also succeeded.

**Unrun**: `cargo check`, Rust unit tests, Clippy, release build, and native Xvfb/Xephyr WM behavior. This sandbox has no rustc/cargo. The uploaded archive itself also lacks the `assets/` tree and other full-repo build inputs. Do not treat the green Python/provider gates or previous DeepSeek's builds as a compile/release PASS for Pass 8.

## 3. Wayland/X11 packaging truth

The X11 `.desktop` session remains registered in `packaging/install-manifest.txt` → `/usr/share/xsessions/flamewm.desktop` and points at `/usr/bin/flamewm-session`. **No native Wayland compositor/backend is present** and no misleading Wayland session file was installed. A proper Wayland session requires a compositor/platform provider with window lifecycle, input, output, popup/subsurface and rendering integration, and native tests, not just a new session selector entry.

## 4. Next verification on full Linux checkout

1. Apply the Pass 8 changed source paths onto the latest full tree with original `assets/` (do not wholesale replace the live repository or discard dirty changes).
2. Run `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` and `RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test --workspace`; independently verify added Rust tests. Run release build and project guards.
3. Test on isolated Xvfb/Xephyr: right-click a file, click blank desktop to dismiss, update theme/selection and switch workspaces repeatedly; ensure no menu reappears and no invisible context action rectangle remains. Test Convert wheel and cancel/close paths.
4. Test `.gif` → `.jpg`, `.png`, `.webp` and collision-free output names with two-frame sources, including failure/timeout/missing delegate and concurrent conversions. Confirm source untouched and no hidden `.flamewm-convert-*` debris remains.
5. Test held-window right/left edge crossings on 4, 5, and 6 workspaces: mouse wrap is row-local, no repeated zero-timeout wake on a one-cell row, continuous pointer grab and 500 ms cooldown preserved.
6. Build/install `.deb`, select FlameWM **X11** session in display manager. Wayland remains blocked until its real compositor backend is implemented.

## 5. Delivery boundaries

Only five inherited Rust files were edited: `crates/flamewm-desktop/src/main.rs`, `crates/flamewm-desktop-core/src/conversion.rs`, `crates/flamewm-wm-x11/src/wm/mod.rs`, `crates/flamewm-wm-x11/src/wm/pointer.rs`, `crates/flamewm-wm-x11/src/wm/tests.rs`. Added one provider regression test and this report. All existing Pass 7 archive entries are retained (including symlinks and executable permissions). No fake asset tree, binary, `.deb`, or Wayland session was generated.
