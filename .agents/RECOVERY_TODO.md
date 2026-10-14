# Recovery TODO - FlameWM 0.0.7

## Baseline Capsule

- Classification: `FAILED_CONTINUATION`.
- HEAD: `bdc92a2a738c1278d243455f94e8494d62e450f7` (`chore: refresh release manifest`).
- Version: `0.0.7`; no version change authorized.
- Live snapshot: 48 tracked paths plus 127 untracked paths, 175 total paths. Porcelain entries are not path totals.
- Source law: current dirty source wins. Preserve all existing work. No stash, reset, clean, checkout, or restore.
- Governance: `.agents/README.md`, `.agents/CONTRACTS.md`, `.agents/JOBS.md`, and `.agents/PROTOCOL.md` require bounded work and prohibit unexecuted build/test claims. Architecture ownership is `docs/ARCHITECTURE.md` and `docs/CANONICAL_OWNERS.md`.

## Protected Contracts

| ID | Contract | Status | Evidence / next proof |
|---|---|---|---|
| P01 | Wallpaper renders. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P02 | Bitmap watermark is bottom-right. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P03 | Dock defaults to bottom. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P04 | Dock geometry receives real x/y. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P05 | Start button is leftmost. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P06 | Clock typography/location remains close to target. | UNVERIFIED | Prior ledger statement only; no live runtime evidence. |
| P07 | Renderer module split remains split. | UNVERIFIED | Dirty renderer split needs fresh source and build verification. |
| P08 | Real XDG discovery exists. | UNVERIFIED | Dirty Linux integration path needs fresh verification. |
| P09 | Managed-window, workspace, display, and control authorities exist. | UNVERIFIED | Dirty WM/control/reactor paths need fresh verification. |
| P10 | Desktop inotify is reactor-driven; no poll loop. | UNVERIFIED | Dirty reactor/desktop paths need fresh verification. |
| P11 | Contract definition not present in supplied capsule or live governance. | BLOCKED | Recovery owner must supply source-backed P11 definition. |
| P12 | Contract definition not present in supplied capsule or live governance. | BLOCKED | Recovery owner must supply source-backed P12 definition. |
| P13 | Contract definition not present in supplied capsule or live governance. | BLOCKED | Recovery owner must supply source-backed P13 definition. |

## Dirty Overlaps

All entries below are `KEEP`: attribution, semantic completeness, and verification are not established. `REPAIR` requires a fresh failure tied to one owner. `BLOCK` means dependent work must not start.

| Area | Exact dirty overlap | Classification | Recovery handling |
|---|---|---|---|
| J01 surface/reactor primitive | `crates/flamewm-render-x11/Cargo.toml`, `crates/flamewm-render-x11/src/lib.rs`, `crates/flamewm-render-x11/src/xlib.rs`, `crates/flamewm-render-x11/src/app/`, `crates/flamewm-render-x11/src/config.rs`, `crates/flamewm-render-x11/src/ffi/`, `crates/flamewm-render-x11/src/runtime.rs`, `crates/flamewm-render-x11/src/surface_controller.rs`, `crates/flamewm-reactor/Cargo.toml`, `crates/flamewm-reactor/src/lib.rs`, `crates/flamewm-ui-x11/` | KEEP | Fresh containment and contract verification before J13. |
| J02 clock/date | `crates/flamewm-shell-core/src/clock.rs`, `crates/flamewm-shell-core/src/lib.rs` | KEEP | Fresh clock contract verification before consumers. |
| J03 workspace page | `crates/flamewm-shell-core/src/workspaces.rs`, `crates/flamewm-shell/src/taskbar/`, `crates/flamewm-shell/src/projection.rs` | KEEP | Fresh typed workspace projection verification. |
| J04 desktop label/drag | `crates/flamewm-desktop-core/src/layout.rs`, `crates/flamewm-desktop-core/src/presentation.rs`, `crates/flamewm-desktop/src/projection.rs`, `crates/flamewm-desktop/src/main.rs`, `crates/flamewm-desktop/build.rs`, `crates/flamewm-desktop/Cargo.toml`, `ui/desktop/desktop.css`, `ui/desktop/index.html` | KEEP | Fresh pure-contract verification before J14/J15. |
| J05 GetSystem | `crates/flamewm-control-core/protocol/com.arkflame.FlameWM1.xml`, `crates/flamewm-control-core/src/lib.rs`, `crates/flamewm-control-core/tests/platform_control.rs`, `crates/flamewm-control-dbus/src/lib.rs`, `crates/flamewm-control-wire/src/lib.rs`, `crates/flamewm-platform/src/host.rs` | KEEP | Fresh protocol round-trip verification before J13/providers. |
| J06 image slot | `crates/flamewm-render-core/src/model.rs`, `crates/flamewm-ui-x11/` | KEEP | `model.rs` has staged and unstaged hunks; do not mutate without attribution. |
| Shell/UI continuation | `crates/flamewm-shell-core/src/start_menu.rs`, `crates/flamewm-shell-core/src/status.rs`, `crates/flamewm-shell-core/src/taskbar.rs`, `crates/flamewm-shell/Cargo.toml`, `crates/flamewm-shell/build.rs`, `crates/flamewm-shell/src/lib.rs`, `crates/flamewm-shell/src/main.rs`, `crates/flamewm-shell/src/runtime.rs`, `crates/flamewm-shell/src/start.rs`, `crates/flamewm-skin/`, `crates/flamewm-ui/`, `ui/shell/` | KEEP | J07-J13 need source readback and explicit owners. |
| Settings continuation | `crates/flamewm-settings/Cargo.toml`, `crates/flamewm-settings/build.rs`, `crates/flamewm-settings/src/lib.rs`, `crates/flamewm-settings/src/runtime.rs`, `crates/flamewm-settings/src/view.rs`, `ui/settings.css`, `ui/settings.html` | KEEP | J09 needs fresh visual and build proof. |
| WM migration | `crates/flamewm-wm/Cargo.toml`, `crates/flamewm-wm/src/atoms.rs`, `crates/flamewm-wm/src/chrome.rs`, `crates/flamewm-wm/src/classifier.rs`, `crates/flamewm-wm/src/client.rs`, `crates/flamewm-wm/src/geometry.rs`, `crates/flamewm-wm/src/lib.rs`, `crates/flamewm-wm/src/runtime.rs`, `crates/flamewm-wm/src/wm.rs`, `crates/flamewm-wm-x11/` | KEEP | Do not repair or replace absent source without a separate source-backed job. |
| Workspace/build/governance | `Cargo.toml`, `Cargo.lock`, `FLAMEWM_VERSION`, `tools/verify_rwr_promotion.py`, `tools/architecture_guard.py`, `docs/ARCHITECTURE_EXCEPTIONS.toml`, `docs/CANONICAL_OWNERS.md`, other untracked docs/scripts/assets | KEEP | Preserve; verify only through owner-specific jobs. |

## R01-R09 Recovery Status

See `.agents/REGRESSION_LEDGER.md` for owner, cause, and required proof. All are `UNVERIFIED`; existing dirty source prevents an implementation or green claim.

## Job Frontier

| Job | Status | Prerequisite / blocker |
|---|---|---|
| J01-J06 | BLOCKED | J01 and J06 both require `crates/flamewm-ui-x11`; Wave 1 cannot start under existing packet ownership until a fresh audit partitions scopes or serializes ownership. |
| J07-J12 | BLOCKED | J01-J06 contracts are not freshly verified; exact job contracts beyond prior ledger text are absent. |
| J13 | BLOCKED | Requires J01, J05, and J07; shares `crates/flamewm-shell/src/main.rs`. |
| J14-J15 | BLOCKED | Require J04; share `crates/flamewm-desktop/src/main.rs`. |
| J20 | BLOCKED | Prior ledger names `config.rs` provenance drift, but no fresh verifier command/output exists. |

READY frontier: none. Implementation frontier is `BLOCKED`.

## A1 Repair Verification And Source-Audit Blockers

- A1 repair verification did not establish a behavior pass. No implementation claim is authorized.
- J01: local popup coordinates are incomplete in dirty untracked shell UI.
- J02: named pager labels leak and workspace activation route is missing.
- C-START-GROUP: source anchor is absent; work requires unscoped dirty shell/UI ownership.
- Xephyr evidence omits desktop.
- System control is read-only; no revision/action signal exists.
- Calendar projection lacks a marker; safe seam is shell-only after ownership is approved.
- Audio/network contract and provider implementation are incomplete and conflict across untracked integration, WM, and shell migration.
- All 175 dirty paths lack ownership packets.

| Collision group | Exact paths / scope | Recovery effect |
|---|---|---|
| J01 + J06 | `crates/flamewm-ui-x11/` | Not safely disjoint. |
| J01 + J13 | `crates/flamewm-shell/src/main.rs` | Shared shell controller. |
| J04 + J14 + J15 | `crates/flamewm-desktop/src/main.rs` | Shared desktop controller. |
| C-START-GROUP | dirty `crates/flamewm-shell/`, `crates/flamewm-shell-core/`, `crates/flamewm-ui/`, `crates/flamewm-ui-x11/`, `ui/shell/` | No source anchor or approved owner boundary. |
| Audio/network | untracked `crates/flamewm-integrations-linux/`, `crates/flamewm-wm-x11/`, shell migration paths | Contract/provider ownership conflicts. |

Safe next action: establish approved path ownership / continuation boundary then run a stable baseline gate.

## Known Gate Evidence

- LEDGER-GREEN-UPDATE (source-wins, ledger only; no source semantic change; no build/test run by this step).
- FULL-GATE-RERUN-3: `fmt` exit 0, `check` exit 0, `static` exit 0, `arch` exit 0.
- LINUX-TEST-VERIFY: 6 pass.
- FULL-TEST-RERUN: exit 0, ~243 pass.
- Provider / control / system-action lanes: green at unit level only.
- P0-A00-A01 capture: desktop + panel mapped / painted / non-black; black screen NOT reproduced on current source.
- User report: Xephyr renders PERFECT (visual confirmation; not runtime screenshot proof).
- Next frontier is shell/UI convergence: numeric pager caller, SystemChanged refresh, popup local coords, calendar marker, audio/network projection, Xephyr three-process/canary.
- No final DONE claimed; no runtime screenshot proof claimed beyond the above.

## FINAL-LEDGER (source-wins, ledger only; no source change; no build/test run by this step)

- UI-WAVE-GATE-FINAL: 6/6 PASS — `fmt` 0, `check` 0, `test` 0, `static` 0, `arch` 0, `bash-n` 0.
- RELEASE-BUILD: exit 0; artifacts `target/release/flamewm`, `target/release/flamewm-desktop`, `target/release/flamewm-shell`.
- XEPHYR-CANARY: PASS exit 0; non-black screenshot mean `13115.9`.
- User confirms Xephyr renders PERFECT (visual confirmation; not runtime screenshot proof).
- SHELL-UI-WAVE-VERIFY: PASS.
- R01-R09: CLOSED per SHELL-UI-WAVE-VERIFY PASS (implementations in current dirty source win; see REGRESSION_LEDGER FINAL-LEDGER mapping).
- P01-P10: UNVERIFIED pending read-only VERIFY job; P11-P13: BLOCKED (no source-backed definition).
- Residuals NOT claimed: `xdotool` missing so grab proof unavailable; per-window Map State not captured; no phase log markers; Desktop `1920x1080` oversized vs `1350x641` noted.

## 2026-09-07 Handoff Capsule (ledger only; no source change; no build/test run by this step)

- Source identity: version `0.0.7` (`FLAMEWM_VERSION`), HEAD `bdc92a2` (`chore: refresh release manifest`), no remotes configured.
- Dirty classification: `DIRTY/FAILED_CONTINUATION`. Live `git status --short` = 97 porcelain entries (≈55 tracked modified incl. 7 deleted `flamewm-wm` sources + staged/unstaged `model.rs` hunks, ≈40 untracked paths/dirs). Prompt estimate (~45 modified + ~40 untracked) is approximate; current source wins.
- Last known green gates: NONE as live proof. Prior planner static PASS claims (fmt/check/static/arch, ~243 tests, Xephyr canary mean `13115.9`, user PERFECT report) are recorded history only, not re-executed by this step.
- R01-R09: summarized from ledger (popup clip, clock/date, workspace projection, panel token, selection rect, drag session, label bounds, image-slot bridge, media visibility). R10-R14: named by handoff only; no source-backed definitions in live governance — recorded as BLOCKED pending recovery-owner definitions. No R10-R14 implementation or green claim.
- P01-P10: UNVERIFIED (wallpaper, watermark, dock bottom, dock x/y, start leftmost, clock typography, renderer split, XDG discovery, WM/display/control authorities, inotify-driven desktop). P11-P13: BLOCKED (no definition). P14: named by handoff only, no source-backed definition — BLOCKED.
- Waves J01-J11: prior wave scopes overlap dirty paths (J01 surface/reactor, J02 clock, J03 workspace, J04 desktop, J05 GetSystem, J06 image slot, J07-J11 shell/UI/settings/WM continuation). All KEEP, implementation frontier BLOCKED per collision groups above.
- J12 plan: read-only VERIFY job only — record exact commands/exits for P01-P10 and R01-R09 closure mapping; no source mutation; no DONE claim from inspection.
- Law restated: preserve all dirty work. No stash/reset/clean/checkout/restore. No build/test success claimed without execution.

## 2026-09-07 Continuation Handoff v62 (ledger only; no source change; no build/test run by this step)

### 1. Handoff identity

- Handoff: continuation recovery handoff v62 (FlameWM 0.0.7, archive `1630_7_9_26.tar.gz`).
- Date: `2026-09-07`. HEAD: `bdc92a2a`. Branch: `main`. Workspace version: `0.0.7`.

### 2. R01-R11 requirements

| ID | Requirement | Status |
|---|---|---|
| R01 | Taskbar first paint. | UNVERIFIED |
| R02 | Workspace context anchor. | UNVERIFIED |
| R03 | Desktop selection. | UNVERIFIED |
| R04 | Entry icon transparency. | UNVERIFIED |
| R05 | Folder/entry context menu. | UNVERIFIED |
| R06 | Blank desktop context menu. | UNVERIFIED |
| R07 | Calendar circle. | UNVERIFIED |
| R08 | Popup/window rounded black pixels. | UNVERIFIED |
| R09 | Window chrome fidelity. | UNVERIFIED |
| R10 | Cursor role regression. | UNVERIFIED |
| R11 | Compile warnings + segfault. | UNVERIFIED |

All R01-R11 are UNVERIFIED; no green gate has run in this execution.

### 3. Protected contracts P01-P13

| ID | Contract | Status |
|---|---|---|
| P01 | Independent Rust. | UNVERIFIED |
| P02 | RustWebRender oracle. | UNVERIFIED |
| P03 | Renderer/SurfaceRuntime split. | UNVERIFIED |
| P04 | Shell/x11 shell ownership. | UNVERIFIED |
| P05 | Desktop ownership. | UNVERIFIED |
| P06 | Render-core ownership. | UNVERIFIED |
| P07 | Render-x11 ownership. | UNVERIFIED |
| P08 | Icons authority. | UNVERIFIED |
| P09 | Xephyr fail-fast. | UNVERIFIED |
| P10 | No fake state. | UNVERIFIED |
| P11 | No broad cleanup. | UNVERIFIED |
| P12 | No reset/stash/clean. | UNVERIFIED |
| P13 | Preserve dirty work. | UNVERIFIED |

Protected contracts P01-P13 all UNVERIFIED at ledger level (GOV is a constant; no live runtime proof in this step).

### 4. Current source state classification: DIRTY/FAILED_CONTINUATION

- Classification: `DIRTY/FAILED_CONTINUATION`.
- Deleted crates `flamewm-wm` superseded by untracked `crates/flamewm-wm-x11` (SOURCE_DRIFT: `docs/CANONICAL_OWNERS.md` still names the old `crates/flamewm-wm` path).
- `cargo clippy` NOT installed; `rustc 1.93.1`.

### 5. Current READY frontier

- Wave 1:
  - J01 — first-present shell `main.rs` + `runtime.rs`.
  - J02 — xephyr scripts `scripts/xephyr`.
  - J03 — warning cleanup (convergence needed vs J01 on shell/`runtime.rs`).
  - J04 — desktop interaction.
- Wave 2 — renderer transparency (serialized J05).
- Wave 3 — J06 calendar, J07 cursor, J08 desktop menus/selection, J09 shell context anchor.
- Wave 4 — J10 window chrome.
- Wave 5 — proof.

### 6. Next gate (Wave 1 gate)

- `tools/architecture_guard.py`
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo build --release -p flamewm-wm -p flamewm-desktop -p flamewm-shell`
- `FLAMEWM_XEPHYR_CANARY=1 ./xephyr`

Law restated: no green gate is claimed — none has run in this execution. Preserve all dirty work; no stash/reset/clean/checkout/restore.
