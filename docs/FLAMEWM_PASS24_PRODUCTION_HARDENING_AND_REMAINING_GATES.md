# FlameWM 0.1.0 — Pass 24: monitor, MIME and drag-preview hardening

## Baseline / scope

Pass 23 source archive, NOT the user's complete checked-out repository. The export intentionally lacks the full `assets/` tree. Do not delete installed or custom assets, alter icon themes, or overwrite the user's dirty local changes unintentionally. FlameWM is a standalone **Rust/X11** WM. The native SDDM X11 entry and `scripts/build-deb` are preserved. No Wayland support was created.

## Changes applied in source

1. **RandR disconnected-CRTC false monitor:** `crates/flamewm-wm-x11/src/lib.rs::display_snapshot` no longer assigns the root-sized rectangle to every connected output whose CRTC is disabled. A zero-CRTC Xephyr fallback is restricted to an exactly single-output RandR snapshot whose advertised mode equals the root dimensions. A RandR primary that has become inactive no longer prevents deterministic reassignment to the first valid active connector. This targets duplicate/full-screen output geometry that could make monitor 2 switch or overlap display 1.
2. **Phantom taskbars:** `crates/flamewm-platform/src/services/panels.rs::sync_outputs` filters out connected but geometrically inactive outputs. The output remains visible to display diagnostics/settings, but cannot own a real panel until it has nonzero geometry. Unit regression exercises disabled → enabled → disabled output transitions and panel count.
3. **Desktop ghost opacity:** the retained renderer paints node children individually. Setting `opacity:0.5` on `desktop-drag-ghost` **does not alpha-composite its Image and Text descendants**. `projection::project_drag_ghost_icon` now fades only a copied drag image's straight-alpha channel, preserving the shared icon cache. Ghost label nodes receive explicit half-alpha text color. Regression checks 255→128, 128→64, 0→0 without color-keying or altering RGB. **This improves visual correctness but does not certify that the user-reported invisible preview is resolved**; that requires native event/layout/damage/capture tracing.
4. **KDE MIME default opening:** `flamewm_applications::open_target` is a shared, reaping, argv-safe opener for files and URIs. On systems that have KDE KIO launchers it chooses `kde-open6`, `kde-open5`, `kioclient6 exec`, or `kioclient5 exec`; otherwise it retains `xdg-open`. `flamewm-desktop` and `flamewm-x11-desktop` now share the opener. This uses existing KDE user MIME associations and can invoke the KDE Open With chooser for unassociated content; it does **not** overwrite `mimeapps.list` or install missing apps.

## Explicitly open / NOT fixed

- **Drag actual visibility/follow:** in spite of a corrected opacity contract, the live user has repeatedly reported that the preview is completely invisible. A native X11 reproduction and tracing are mandatory. Check MotionNotify→active index→`handle_item(Motion)`→`show_drag_ghost`→`RuntimeDocument::is_effectively_visible`→layout rect→paint commands→present/damage. Capture the exact first failed gate, then repair it; do not stack unmeasured local fixes.
- **Monitor draggable square cards / native layout persistence:** `DisplayPort` exposes `display_snapshot` and `apply_mode`, **not** a real output-position transaction. Implement RandR CRTC topology authority, validate framebuffer bounds, preserve output mode/rotation, stage transaction with 15-second Keep/Revert rollback, wire typed control/wire/DBus, then replace settings' output list with a retained draggable relative-layout view. A static draggable mock is not a solution and no fake layout editor was added.
- **Browser password store and KDE Wallet:** the browser's encrypted profile and wallet may rely on the original Plasma session's KWallet/Secret Service. Service ownership, PAM unlock, and browser's `--password-store` backend must be observed on the **same account** before choosing integration. **Never copy/rewrite/clear wallet files or browser login stores.** No claim of working sync or automatic unlock.
- **Intermittent dual-monitor session outage and right-click freeze:** the report is not supported by attached WM/shell/display traces or a deterministic reproduction. The invalid-CRTC and phantom-panel mechanisms are real source defects but do not establish that they were the only cause.
- **Production certification:** native Rust compilation, Xephyr and SDDM dual-monitor long-session testing were unavailable in the authoring environment (no rustc/cargo/Xephyr). Do not declare a production-ready release.

## Tests and verification gates

- Offline: `python3 -I -m unittest discover -s tools -p 'test_flamewm_pass24_regressions.py' -q`; also run `tools/test_flamewm_xtest_drag.py`, `tools/test_flamewm_workspace_edge_acceptance.py`, and `python3 -I tools/ui_contract_guard.py`.
- In the complete Linux checkout, first compile/test exact owners: `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && cargo test -p flamewm-platform -p flamewm-applications -p flamewm-desktop -p flamewm-wm-x11`.
- Run `./xephyr` and exercise an unplugged/disabled attached monitor, two active side-by-side monitors, a monitor left of origin, desktop drag across tiles with visible 50%-opacity icon/label, regular file open (associated and unassociated), KDE Open With, and right-click after changing monitor mode.
- For native wallet verification, collect **metadata only**: environment `XDG_CURRENT_DESKTOP`, DBus service names, whether `kwalletd6`/`org.freedesktop.secrets` exists, and selected browser backend. Do not log secrets or wallet contents.
- Package only from the **full** repository: `./scripts/build-deb` and install via `sudo apt install --reinstall ./.target/dist/flamewm_0.1.0_amd64.deb`. Confirm `FLAMEWM-DEB-READY`, `dpkg-deb -f ... Architecture` prints `amd64`, and `/usr/share/xsessions/flamewm.desktop` exists.

## Protected contracts

No changes to workspace count / navigation, popup animations, panel opacity, titlebar glyph fallback, icon-theme restriction (PlagueSur / Fluent), existing X11 SDDM session, package control metadata, password files, MIME defaults, unassociated file contents or Wayland. Changes confined to small canonical owners and their tests.
