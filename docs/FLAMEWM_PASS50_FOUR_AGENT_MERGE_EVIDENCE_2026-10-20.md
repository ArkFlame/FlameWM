# FlameWM Pass 50 — Four-Agent Patch Integration / Source-Evidence Report

**Baseline:** FlameWM 0.1.0 Pass 49 integrated source overlay. **Input:** `flamewmpatches1905.tar.gz` containing exactly four agent patches. Four patch applications were checked with `git apply --check` and applied without hunk conflicts. This report describes source-level integration, **not native-build or visual runtime certification**.

## Scope and merged implementation

- **Agent 1 — scale**: 21 values from 100 through 200 percent in 5-percent steps, Settings native slider and release commit, persistence/validation and per-output display identity, shell/panel projection, icon cache handling. Source: `flamewm-platform`, `flamewm-settings`, `flamewm-shell`, `flamewm-ui-core`, UI markup/CSS.
- **Agent 2 — desktop and cursor**: desktop icon/label scaling; modern desktop label text shadow with renderer-owned blur radius; Xcursor silhouette shadow and user toggle; shared render compiler, document codec (v7), paint and X11 backends.
- **Agent 3 — themes**: active icon-theme thumbnail beside selected value; curated McMojave/Oxygen cursor-family list and McMojave-first resolution if installed; cursor packaging license guard and Xephyr fallback. Other OS themes are not deleted and saved preferences remain valid.
- **Agent 4 — titlebars**: Windows 11-style full-cell hover painting, antialiasing, control hit-area/corner constraints, WM frame geometry and title glyph/titlebar metrics. Existing circle style remains available.

## Integration repairs beyond the four patches

1. Applied the `cursorShadow` setting through the **existing per-process cursor owner registries** for Desktop, Shell/quick-control helper, Settings, and WM (with and without the XSETTINGS manager). Reused `flamewm-ui-x11` / `flamewm-wm` facades rather than adding the forbidden renderer dependency to the desktop product crate. This returns the architecture guard to its seven pre-existing source violations; no new ones from this integration.
2. The selected icon-theme thumbnail now uses `ThemeRegistry::selected_preview` so an empty saved icon-theme setting correctly resolves to the default theme thumbnail.
3. Guarded null Xcursor image/pixel allocations before native pixel writes, retaining failure cleanup.
4. Corrected two obsolete Python source-contract assertions to test the current blurred text-shadow wire version and the existing factored slide controller, not obsolete identifiers.
5. Added `tools/test_flamewm_pass50_integration_contract.py` for static cross-agent integration checks.

## Executed verification

- `git apply --check` for each of the four patches on the baseline: **PASS**; all four applied cleanly.
- `python -m unittest -q` with 20 selected source, packaging, settings, screenshot, previous memory-pass and new integration suites: **308 passed, exit 0**. Detailed selected suites logged in the merging process.
- Prior to final integration repairs, the same selected suites reported two stale assertions; these were corrected after tracing the newer source contracts.
- `python tools/architecture_guard.py`: **exit 1, seven pre-existing violations** (`flamewm-applications` three, `flamewm-desktop/src/projection.rs` one, `flamewm-screenshot/src/main.rs` one, `flamewm-wm/src/session_app_compat.rs` one, `ui/desktop/desktop.css` one). Two extra violations from Agent 2's direct renderer dependency were resolved by moving the caller to the existing UI backend facade. These seven need independent architecture cleanup; not hidden as passing.

## NOT RUN / remaining acceptance blockers

- **Native Rust/Cargo**: this archive was prepared in a container without `cargo`/`rustc`; no `cargo check --workspace --all-targets`, Rust library tests, Debian build, linkage or ABI checks were possible. These checks are required before this can be called a fully working release.
- **Runtime graphics**: no real X11 or dual-output Xephyr window session, theme raster preview, cursor-shadow or WM titlebar acceptance screenshot was taken. Test the hovered cell's upper-right rounding on both rounded and squared decorations; check hit testing and live scale changes with multiple physical outputs.
- **Global scaling limitation:** the underlying X11 WM still seeds `WmConfig::titlebar_height` from `FLAMEWM_TITLEBAR_HEIGHT` at startup; the per-output shell scale currently does not dynamically reconfigure every managed window's native frame extents/titlebar height. Likewise, third-party application toolkit scaling is not guaranteed by the Flame-owned shell slider. These are **PARTIAL**, not fulfilled universal-KDE-parity claims; require a properly designed WM/display-scale state/geometry update and runtime verification.
- **Assets:** the Pass 49 overlay and patches omit complete `assets/` and `.vendor/` trees. This source overlay must be applied over the user's full live checkout. The McMojave cursor is preferred only when a properly licensed cursor theme is installed; otherwise the code falls back to Oxygen or emergency Breeze. No cursor files or licenses were fabricated.

## Safe local build/proof order

Extract this **Pass 50** archive over the complete asset-bearing checkout (after backing up or reviewing existing edits). Do not reapply Pass 49 or older after Pass 50. The source archive contains historical tar member times; `tar --touch` ensures Cargo notices extracted updates.

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS50_FOUR_AGENT_INTEGRATED_SOURCE_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null &&
tar --touch -xzf "$ARCHIVE" -C . &&
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets &&
CARGO_BUILD_JOBS=2 cargo test -p flamewm-platform -p flamewm-settings -p flamewm-desktop -p flamewm-render-core -p flamewm-render-x11 -p flamewm-wm-x11 -p flamewm-shell -p flamewm-session-core &&
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Only then perform controlled Xephyr visual QA with mixed-resolution monitors where available. Source and package checks do not prove the live visual behavior. If Cargo reports new errors, repair those specific diagnostics in the full checkout before calling the release complete.
