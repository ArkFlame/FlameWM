# FlameWM 0.1.0 Pass 18 — desktop drag, keyboard Delete, minimal Convert wheel

## Source

Built from the uploaded `FlameWM_0.1.0_pass17_desktop_icons_trash_radial_conversion_2026-10-19.tar.gz`. Patch targets **Pass 17**; merge into the current live checkout cautiously if independent agent changes are present. Not an executable release build.

## User-visible changes

1. **Desktop drag.** Once pointer movement crosses the existing drag threshold, the canonical desktop icons belonging to the drag selection are hidden, and one 50%-opacity icon+label ghost follows the pointer via the retained `desktop-drag-ghost` document node. A drag across blank desktop content continues routing to the anchor; on release/cancellation the source item visibility is restored, and successful release commits to the existing grid/layout transaction. The icon remains in the desktop model during the gesture; no filesystem move or layout commit happens until release.
2. **Delete/Supr to Trash.** The desktop controller now consumes the real `UiControllerEvent::Key` instead of discarding typed keys. An unmodified `Delete` press on a selected desktop file opens the existing *Move to Trash?* confirmation, then the existing `Trash::move_to_trash` provider. The UI's legacy `keyboard.input` bridge remains as a fallback but does not reopen an already-pending confirmation. Rename and sticky editing are excluded; Ctrl/Alt/Super chords are not treated as desktop Delete. The shared native X11 event owner grants the **desktop surface**, not ordinary app windows or panels, input focus on primary/secondary press so selected file shortcuts can receive key events. No global Delete passive grab is installed.
3. **Convert wheel.** Removed the eight permanently painted petal asset and all peach hover pills. Wheel background, opacity, backdrop blur and text adapt to FlameWM settings; the circle retains its radial shape when other menu surfaces use square decoration. White format labels remain legible on a configurable translucent surface, and a muted accent hub displays file size. Only the hovered fraction is rendered in the current accent color. One canonical annulus/sector predicate powers both the visual mask and the full-sector click routing, including pointer clicks outside labels. Hover masks are computed once via `OnceLock` with antialiased boundaries and tinted on demand. Eight file formats remain provider-selected; absent slots remain hidden.
4. **Ten browser-only visual prototypes.** Standalone HTML/CSS/JS prototypes and PNG screenshots are provided separately, not bundled into the native UI or release source. Concept **04 Quiet radial** was chosen for its low-noise surface, absence of dividers and readable accent feedback. Prototype CSS/JS uses browser SVG; native FlameWM uses its retained UI and Rust raster mask, with no WebView or JavaScript at runtime.

## Canonical owners

- `crates/flamewm-desktop/src/main.rs`: typed UI key/controller handlers, selection/drag semantic state, Convert action/state, root sector click routing.
- `crates/flamewm-desktop/src/convert_wheel.rs`: deterministic sector geometry, antialiased cached masks, unit tests.
- `crates/flamewm-desktop/src/projection.rs`: surface fill and radial corner clipping.
- `crates/flamewm-render-x11/src/app/{mod.rs,events.rs}` and `surface_controller/{raw_press.rs}`: desktop-only X11 focus policy using the existing native Xlib focus FFI owner.
- `ui/desktop/{index.html,desktop.css}` and desktop build script/UI contract: strict UI document inventory, no petal artwork dependency.

## Verification and missing gates

- The source-contract tests `test_flamewm_desktop_pass18_contract.py` and updated Pass 17 tests pass offline.
- XTest harness and workspace-edge harness regressions pass, as does the UI contract guard with 0 errors and 24 inherited warnings.
- The full Rust workspace/desktop compile and native Xephyr test **have not run here** (no `rustc`/`cargo` and source export omits necessary packaged assets). Static checks **cannot** establish real drag pixel movement, focus delivery, accurate hover hit testing or surface alpha compositing. A native test must verify these, especially keyboard focus after selecting a desktop file while a separate managed window was active.
- A true percentage-progress conversion fill remains outside this pass: conversion providers do not expose a reliable unified progress percentage.

## Native acceptance in the complete local repo

1. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` and `CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-desktop -p flamewm-wm-x11`.
2. `./xephyr` and confirm WM, desktop, panel all remain running.
3. Hold left-click on a desktop file and drag across several cells and blank desktop. Before release the original tile must be invisible; one 50% ghost with **original icon and full label** must follow the pointer every frame, preserving cursor grip. Release must restore one real tile at the destination; pressing Escape/releasing without threshold restores the original.
4. Select one/two icons, press **Del/Supr**. Prompt must say *Move to Trash*; cancel preserves files, confirm moves selected files through the Freedesktop Trash implementation; no instant permanent delete. Test right-click selection too, then Delete. Test delete inside rename/sticky text editor does not trash desktop entries.
5. Open Convert on PNG: wheel should use surfaceColor/Opacity/Blur and live accent; no static petals, rectangular text hovers or peach backgrounds. Hover across segments and hub, click within an annular wedge well outside the label; only the correct format should convert. Repeat at different DPI scales and screen edges. Change accent and surface opacity while wheel is open; verify update and corner shape.
6. Verify no UI compiler missing-node errors and no regressions to keyboard shortcuts, popup dismissal, maximized-window restoration or file conversion atomic publication.

## Known risk

The normal desktop window must be mapped and viewable for XSetInputFocus; the X server/WM can override its focus as usual. Focus cannot be guaranteed from source inspection alone. The native radial shadow/blur depends on the existing renderer's supported X11 backdrop path; no new compositor/Wayland implementation was introduced.
