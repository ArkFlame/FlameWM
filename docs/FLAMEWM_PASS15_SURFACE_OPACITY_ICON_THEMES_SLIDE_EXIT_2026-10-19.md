# FlameWM 0.1.0 — Pass 15: taskbar opacity, icon theme curation, slide-out reliability

**Baseline:** Pass 14 strict-desktop-CSS archive, **not** the user's live Git checkout. All original source files retained in this archive. This is an isolated source patch and must be reconciled with concurrent local edits; do not overwrite the live tree wholesale.

## Resolved in source

### Taskbar surface opacity on all outputs

`crates/flamewm-shell/src/runtime.rs` held just one primary-panel `panel_snapshot` / `panel_capture`, while the application created a Dock `SurfaceHandle` for each connected output. The wallpaper snapshot and `set_surface_backdrop` were therefore only applied to the primary taskbar. `crates/flamewm-shell/src/app.rs::project_settings` also updated the background color/alpha only in the primary panel document.

* Moved backdrop snapshot and capture key to each `PanelSurfaceRecord`; each output records its own wallpaper/fit/device rect and capture lifecycle.
* Iterate panel records for backdrop refresh, blur/opacity composition, and clearing stale captures.
* Project the same `surfaceColor`/`surfaceOpacity` onto every panel document; redraw all panels rather than only the first one. Reproject settings after monitor topology re-synchronization.
* Preserves no-compositor pseudo-transparency: wallpaper pixels behind a panel are cached/blurred; this is not real-time alpha compositing of arbitrary windows on non-composited X11.

### Curated icon themes

`crates/flamewm-settings/src/theme_picker.rs`: `PlagueSur` and `Fluent` are now the only icon picker entries. Discovered upstream `Fluent Yellow` or `Fluent-Yellow` keeps its real on-disk theme id and preview for lookup, but displays as `Fluent`. Cursor themes remain separate and unaffected. Other icon themes installed outside FlameWM are not deleted from the user's machine. `crates/flamewm-integrations-linux/src/icons.rs` removes the deprecated McMojave fallback (Breeze remains a non-selectable fallback for missing app icons). `scripts/package` no longer copies McMojave-circle or candy-icons; packaging manifest and Debian acceptance reflect the change. The bundled FlameWM Xcursor pack and existing McMojave cursor-theme installation are preserved when their assets are present.

**Asset availability blocker:** The uploaded Pass 14 export has no `assets/themes/icons/PlagueSur` or Fluent icon assets. This patch does not fabricate, redistribute or download those themes. Packaging copies them only if they are present in the full repository and suitably licensed. The installer may therefore need system/user theme packages; validate the actual `index.theme` and icons before promising installed availability.

### Symmetric slide in/out + rounded corners and taskbar occlusion

`crates/flamewm-render-x11/src/app/render.rs` had divergent mask paths: settled frames respected the `ring_mask` / `dock_corner_radius`, while `refresh_shape_mask_clipped` used `mask_spans` unconditionally. A slide could lose the rounded corner mask as soon as clipping began. Both paths now use one `base_shape_spans` owner and the clipped path intersects it with `panel_clip` (which excludes the taskbar overlap).

On slide-out landing, `advance_motion` used to restore a full un-clipped window shape while it was **still mapped**, then issue `XUnmapWindow`, potentially exposing a one-frame flash above the taskbar. Now the close path unmaps first, then restores shape invisibly for the next show; it does not raise the closing popup. Slide-out uses the same 180ms duration as opening and the time inverse of opening easing (`EaseInCubic` closing vs `EaseOutCubic` opening). Retargeting mid-animation still starts at current progress.

## Verification carried out here

* New `tools/test_flamewm_surface_icon_slide_regressions.py`: **8 pass** on Pass 15; baseline negative-control **6 failures / 2 errors** on Pass 14.
* Rust unit tests added for ease-curve reversal, identical clipped/unclipped shape owner, and X11 close order. Updated timeline assertions to the 180ms close duration. **Not compiled in this environment** (no `cargo` / `rustc`).
* `python3 -I -m unittest discover -s tools -p 'test_*.py' -q`: 703 tests, four failures and one skip; the four failures require unavailable Python `dbus` / usable `XDG_RUNTIME_DIR` in the fake-portal tests. See saved test log outside archive. Run the remaining test suite excluding that module to establish a clean unaffected gate.
* `python3 -I tools/ui_contract_guard.py`: 0 errors, 24 inherited warnings.
* `bash -n scripts/package`: passed.

## Still required in user's full local checkout

1. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` and focused Rust tests in `flamewm-reactor`, `flamewm-render-x11`, `flamewm-shell`, `flamewm-settings`.
2. Run `./xephyr` in an isolated `DISPLAY`; set surface opacity to 0%, 50%, 100%, then verify color alpha and backdrop on **both** primary and secondary panels, including after replug/reconfigure. A static background is not a live compositor.
3. Open/close Start, battery, Wi-Fi, calendar and media popups on bottom and top taskbar edges, at high and low speeds. Capture frame-by-frame at close; assert no pixel below the top of the bottom panel, live background in rounded corners, and no full-shape flash before unmap. Confirm behavior when XShape is unavailable is reported explicitly.
4. Verify icon picker lists only PlagueSur and Fluent; check actual theme directory locations and license before bundling. Ensure cursor theme picker remains independent.
5. Check `git status --short` and current diff before applying integration patch. Do not drop the other agent's live edits.
