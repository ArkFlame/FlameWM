# FlameWM 0.1.0 — Pass 19 UI alignment, pointer hover, menu glyphs and Ribbons

## Baseline

Patch target: FlameWM Pass 18 `FlameWM_0.1.0_pass18_desktop_drag_delete_minimal_wheel_2026-10-19.tar.gz`. Preserve workspace, conversion, drag/drop, Trash and animation fixes. This export does not include `assets/` and cannot itself produce a complete release build.

## Resolved from source

1. **Convert wheel label symmetry.** Eight format labels are now centered on one polar ring (radius approximately 113 px from the 164 px wheel center) using 80×32 rectangles. Fonts use weights 650 (options) and 700 (hub). Button and hub geometry is independent of source file type; formats are still assigned by the existing canonical `offered_formats` provider.
2. **Hover delivery on unchanged UI nodes.** The native X11 event dispatcher previously emitted `ActionPhase::Hover` only when the *retained node index changed*. A wheel sector can change while the pointer remains over the same broad node. Passive pointer motion now emits a hover event for the current target every time; only changed node identity forces a repaint. The semantic Convert handler deduplicates unchanged sectors, and every descendant resolves the same `wheel_hit_at` polar predicate for hover and click. A click on an empty sector does not convert.
3. **Context menu glyphs.** Three FlameWM-owned SVG glyphs are staged into the **desktop** compiled RWR under `assets/breeze`: Convert (two arrows), Add Virtual Desktop (screen + plus), Create New Folder (folder + plus). The `Convert` row has its icon on the left, as with other rows. These are shell-owned UI assets, not selectable icon themes. No cursor or user-installed icon packs are modified.
4. **Ribbons lock-screen presentation.** A new `Ribbons::new_fluid_style` constructor uses 0.92 motion speed factor and 1.70 steering curvature for longer visible arcs and flowing colored trajectories on black. It is used **only by FlameWM lock**; default `Ribbons::new` and its deterministic golden-frame expectations are unchanged. This is a Windows-11-*inspired* source preset, **not** a pixel-identical recreation; native screenshots and tuning are still required.

## Supplied D-Bus log excerpt (not evidence of a FlameWM crash)

- `org.freedesktop.portal.Desktop`, `Documents`, `PermissionStore`, GTK and KDE backend **activated successfully**. Multiple providers being activated can be normal in a nested desktop environment; this is not proof of a FlameWM defect.
- `qt.qpa.services: Failed to register with host portal ... Connection already associated with an application ID` occurred in a Qt application (`kcalc`) rather than in the FlameWM processes. It suggests repeated/competing Qt portal app-ID association on the same session connection. No WM-side source repair is justified without application, portal, and host logs.
- `kf.wallet.ksecretd: Lacking a socket, pipe: 0 env: 0` is an environment/session issue in KDE Wallet activation; no evidence in the supplied excerpt establishes that the WM or desktop crashed or lost persistence. No destructive portal/KWallet configuration changes were applied.
- The five absolute `.target/flamewm/20261019T233020Z-2169070/*.log` paths supplied by the user refer to their Linux system. They are **not mounted** in the authoring sandbox; this review did not inspect their contents. To diagnose actual process errors, upload the five files or their error lines from the full session. Do not treat portal activation lines as FlameWM failures.

## Verification

- New source-contract tests `tools/test_flamewm_pass19_wheel_icons_ribbons.py` (6 passes): wheel center geometry and weight, continuous native hover dispatch, unified hover/click sectors, three build-staged menu SVGs, and lock-only ribbon preset.
- Existing focused test suites: Pass 17 contracts 7, Pass 18 contracts 6, XTest harness 53, workspace-edge harness 7, icon/surface/slide 8, graphical flicker 6, conversion provider 2, source archive 4. Total focused tests including new tests: **99 passing**.
- `python3 -I tools/ui_contract_guard.py`: **0 errors, 24 inherited warnings**.
- Every custom SVG is valid XML with a 24×24 viewBox.
- **Not run:** Rust compilation, Rust unit tests, RWR compiler build-script execution, native X11/Xephyr click/hover checks and lock-screen live Ribbons appearance; `cargo`/`rustc` are absent in this authoring environment. These must not be described as native-verified.

## Native acceptance on the full repository

1. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`
2. `RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-ribbons -p flamewm-lock -p flamewm-desktop -p flamewm-render-x11`
3. `./xephyr` with the full `assets/` tree. Right-click a convertible file and move slowly within one native button rectangle: the hovered wedge must update at each polar crossing. Clicking an entire wedge must convert to the selected format, clicking the hub must do nothing.
4. Verify format names appear at equidistant cardinal/diagonal positions. The three context-menu glyphs should be complete and left-aligned.
5. Open the Ribbons screensaver with default settings; compare live trajectories (and CPU cost) against the supplied Windows reference. Check both 720p and 1080p and retain screenshots before declaring parity.
6. Run the same Xephyr session under the user-installed KDE/GTK portal environment and capture portal warnings alongside actual FlameWM process logs. Do not conflate application portal warnings with FlameWM failures.

## Guardrails

Preserve `ui/desktop/index.html` compiled dynamic label IDs, layout/paint authority, conversion worker atomic output logic, menu dismissal and workspace ownership. Avoid adding provider polling, fake progress percentages, feature-specific raw X11 widgets or new icon-theme dependencies.
