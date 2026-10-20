# FlameWM 0.1.0 Pass 35 — Xephyr panel readiness and output-reconnect state

## Baseline and observable failure

Source: uploaded `FlameWM_0.1.0_PASS34_MEMORY_PACKAGING_FIXED_2026-10-20.tar.gz`.
Evidence: supplied `logs.tar.gz`, plus user terminal output from `scripts/xephyr`.

- `wm.log`: WM initialized, published its X11 identity, no unrecoverable error before harness shutdown.
- `desktop.log`: desktop initialized; final X connection closed by harness shutdown.
- `shell.log`: `flamewm-shell: GetPanels returned no panel`.
- This means the immediate startup failure is an **empty control-plane panel snapshot**, not a compositor crash. The logs do **not** include the RandR output snapshot, so which exact Xephyr RandR variant was returned is not directly proven by the logs.

## Source-proven defect and fix

`X11Desktop::display_snapshot` already provided a root-sized virtual output with `current_mode = ModeId(0)` when RandR returned no outputs. `PanelService::sync_outputs` unconditionally excluded every mode-zero output, even one with a valid drawable root rectangle. The result was zero taskbars and a fatal `ShellSurfaces::create` error. For a single connected output with CRTC zero and no *matching advertised mode*, the X11 adapter also discarded its valid root geometry entirely.

Pass 35 changes only this pipeline:

1. `crates/flamewm-platform/src/services/panels.rs`: a connected output with **valid geometry** owns a panel even if RandR does not advertise a mode ID. Disabled/disconnected or zero-sized outputs remain excluded. Rust regressions cover both paths.
2. `crates/flamewm-wm-x11/src/lib.rs`: a lone RandR output may use the real X root rectangle when CRTC is zero, even if no RandR mode matches the root. Multi-output disabled CRTCs never receive this fallback. Real matching mode IDs are preserved. Rust regressions check fallback, multi-output refusal and real-mode selection.
3. `crates/flamewm-control-core/src/lib.rs`: `GetShellBootstrap` now refreshes the display and reconciles the authoritative panel service **before** returning a coherent display/panel snapshot. A regression verifies output coordinate changes reach the initial panel snapshot.
4. `crates/flamewm-platform/src/host.rs`: when a monitor disconnects/reconnects, recreate panels using persisted edge, height and floating-frame settings, not unstyled defaults. A regression covers disconnect/reconnect.
5. `crates/flamewm-shell/src/runtime.rs`: the existing fatal no-panel condition now includes a diagnostic of each output's connected state, mode and geometry. It still rejects genuinely headless/no-drawable-output sessions; no fake panel is created.
6. `tools/test_flamewm_pass24_regressions.py`: updated two source-contract assertions to match the accepted new output validity rule, **retaining** explicit disabled/disconnected monitor rejection tests.
7. `tools/test_flamewm_pass35_xephyr_panel_bootstrap.py`: new focused offline source regression checks.

### Protected contracts

- No change to taskbar styling/assets, cursor startup indicator, Start menu, MIME/file associations, KWallet, saved Brave profiles or credentials.
- No fabricated RandR *display mode*: `ModeId(0)` still means unavailable for mode selection.
- No phantom panel for disconnected or geometryless outputs.
- Multi-monitor geometry remains owned by the WM/XRandR; the shell consumes reconciled snapshots.
- No new long-running process or per-frame polling. These are event/startup-path fixes.

## Verification

Offline source-level tests executed in this environment:

```text
python3 tools/test_flamewm_pass35_xephyr_panel_bootstrap.py -q
python3 tools/test_flamewm_pass34_release_memory_contract.py -q
python3 tools/test_flamewm_pass29_cargo_targets.py -q
python3 tools/test_flamewm_pass24_regressions.py -q
python3 tools/test_flamewm_pass23_production_contracts.py -q
```

Rust `cargo check`, Rust unit tests, Xephyr and graphical UI tests are **not available in this environment**, so the fix is source-supported but not runtime-certified. The user's previous Pass 34 Cargo build succeeded; that fact does not certify Pass 35's changes.

## Run on the actual development machine

Extract Pass 35 over the existing complete FlameWM checkout. Do not extract an older Pass afterward.

```bash
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-platform -p flamewm-control-core -p flamewm-wm-x11 && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb && \
./scripts/xephyr
```

Acceptance: the shell stays running and displays a panel in 1350×641 Xephyr; `GetPanels` yields a real panel; EWMH/Desktop/Start continue working. Also test physical dual-output arrangements, disconnection/reconnection and persisted taskbar edge and height. If a new no-panel message remains, it now includes the detailed display snapshot. Capture the current `shell.log`, `wm.log` and `DISPLAY=:97 xrandr --verbose` on the live Xephyr display for further discrimination.

## Remaining, not claimed fixed

Brave/KWallet credential continuity, native GTK/Qt/portal app association behavior, multi-monitor real hardware interactions, Plasma-equivalent device management, Wayland, and existing architecture-guard findings remain outside this narrow failure repair. No credentials, desktop settings files or user data were edited or migrated.
