# Regression Ledger - FlameWM 0.0.7

Baseline: `FAILED_CONTINUATION`, HEAD `bdc92a2a738c1278d243455f94e8494d62e450f7`, version `0.0.7`, 48 tracked paths plus 127 untracked paths, 175 total paths. Porcelain entries are not path totals. LEDGER-GREEN-UPDATE only: source wins, bounded ledger update, no source semantic change, no build/test run by this step. FULL-GATE-RERUN-3: fmt 0, check 0, static 0, arch 0. LINUX-TEST-VERIFY: 6 pass. FULL-TEST-RERUN: exit 0, ~243 pass. Provider/control/system-action lanes green at unit level only. P0 black-screen NOT reproduced on current source (desktop+panel mapped/painted/non-black) plus user report Xephyr renders PERFECT. No final DONE; no runtime screenshot proof beyond what exists. Next frontier: shell/UI convergence (numeric pager caller, SystemChanged refresh, popup local coords, calendar marker, audio/network projection, Xephyr three-process/canary).

| ID | Contract / first bad state from prior ledger | Canonical owner | Dirty overlap | Classification | Required fresh proof | Status |
|---|---|---|---|---|---|---|
| R01 | Start/tray popup is clipped because one Dock document toggles child visibility. | `flamewm-ui-x11` / `flamewm-render-x11` surface boundary | J01; shell build/main; `ui/shell/` | KEEP | Surface map/raise/hide and outside-event proof. | UNVERIFIED |
| R02 | Clock is one-shot and date format has leading zeros. | `flamewm-shell-core` clock; reactor timer source | J02; shell projection | KEEP | Minute boundary, rollover, and no-leading-zero date proof. | UNVERIFIED |
| R03 | Workspace projection writes raw slots instead of typed active/page state. | Shell taskbar/workspace facade | J03; shell projection; panel CSS/template | KEEP | Visible-page cases for 1, 4, 8, and 18 slots. | UNVERIFIED |
| R04 | Panel token is black and clock region is visually split. | Skin/panel presentation owner | `ui/shell/flamewm.css`, shell/UI continuation | KEEP | Visual proof of one dark-gray panel. | UNVERIFIED |
| R05 | Selection rectangle updates only on motion and renders below items. | Desktop runtime + selection model | J04; desktop runtime/template/CSS | KEEP | Drag motion and post-release selection proof. | UNVERIFIED |
| R06 | Drag session is unwired; persistence is not invoked. | `flamewm-desktop-core` layout | J04; desktop runtime | KEEP | Group transaction, persistence, no-open-after-drag proof. | UNVERIFIED |
| R07 | Label is single-line and unbounded. | `flamewm-desktop-core` presentation | J04; desktop build/template/CSS | KEEP | Long ASCII, Unicode, long-token, and empty-label proof. | UNVERIFIED |
| R08 | Runtime image-slot API/projection is missing. | `flamewm-render-core` and `flamewm-ui-x11` bridge | J06; `model.rs` has staged plus unstaged hunks | BLOCK | Attribute overlapping `model.rs` hunks, then validate RGB8/revision projection. | UNVERIFIED |
| R09 | Media visibility includes stopped state; tray/popup projection is incomplete. | `flamewm-shell-core` status facade | Shell/UI continuation | KEEP | Playing/paused, stopped, available, unavailable visibility matrix. | UNVERIFIED |

## Non-Regression Rules

- Preserve dirty source. `KEEP` does not authorize mutation; `REPAIR` requires a fresh source-proven failure. `BLOCK` requires resolved attribution or prerequisite proof.
- J01 and J06 both require `crates/flamewm-ui-x11`; they are not safely disjoint. Wave 1 cannot start under existing packet ownership until a fresh audit partitions scopes or serializes ownership.
- Do not introduce second WM, popup/surface, renderer, or state authority. Ownership follows `docs/ARCHITECTURE.md` and `docs/CANONICAL_OWNERS.md`.
- No test, build, verifier, visual, or runtime proof is green until its exact command and exit evidence is recorded by a read-only VERIFY job.

## Recovery Blockers

| ID | Discovered blocker | Status |
|---|---|---|
| B01 | J01 local popup coordinates incomplete in dirty untracked shell UI. | BLOCKED |
| B02 | J02 named pager labels leak; workspace activation route missing. | BLOCKED |
| B03 | C-START-GROUP source anchor absent; requires unscoped dirty shell/UI ownership. | BLOCKED |
| B04 | Xephyr evidence omits desktop. | BLOCKED |
| B05 | System control is read-only; no revision/action signal. | BLOCKED |
| B06 | Calendar projection lacks marker; safe seam is shell-only after ownership approval. | BLOCKED |
| B07 | Audio/network contract/provider implementation incomplete; conflicts across untracked integration, WM, and shell migration. | BLOCKED |
| B08 | 175 dirty paths have no ownership packets. | BLOCKED |

Collision groups: `J01 + J06: crates/flamewm-ui-x11/`; `J01 + J13: crates/flamewm-shell/src/main.rs`; `J04 + J14 + J15: crates/flamewm-desktop/src/main.rs`; `C-START-GROUP: crates/flamewm-shell/, crates/flamewm-shell-core/, crates/flamewm-ui/, crates/flamewm-ui-x11/, ui/shell/`; `audio/network: crates/flamewm-integrations-linux/, crates/flamewm-wm-x11/, shell migration paths`.

Implementation frontier: `BLOCKED`. No behavior pass claimed. Safe next action: establish approved path ownership / continuation boundary then run a stable baseline gate.

## FINAL-LEDGER (source-wins, ledger only; no source change; no build/test run by this step)

- UI-WAVE-GATE-FINAL: 6/6 PASS — `fmt` exit 0, `check` exit 0, `test` exit 0, `static` exit 0, `arch` exit 0, `bash-n` exit 0.
- RELEASE-BUILD: exit 0; artifacts `target/release/flamewm`, `target/release/flamewm-desktop`, `target/release/flamewm-shell` (reported path `.target/release/` in job handoff; canonical artifacts under `target/release/`).
- XEPHYR-CANARY: PASS exit 0; non-black screenshot mean `13115.9`.
- User confirmation: Xephyr renders PERFECT (visual confirmation; not a runtime screenshot proof).
- SHELL-UI-WAVE-VERIFY: PASS.
- R01-R09 closure mapping (per SHELL-UI-WAVE-VERIFY PASS; implementations in current dirty source win):
  - R01 popup clip → surface map/raise/hide + outside-event handling in `flamewm-ui-x11` / `flamewm-render-x11` boundary — CLOSED per verify PASS.
  - R02 clock/date → minute-boundary ticker + no-leading-zero date in `flamewm-shell-core` clock — CLOSED per verify PASS.
  - R03 workspace projection → typed active/page state in shell taskbar facade — CLOSED per verify PASS.
  - R04 panel token → single dark-gray panel token in skin/panel owner — CLOSED per verify PASS.
  - R05 selection rect → motion update + above-items render in desktop runtime — CLOSED per verify PASS.
  - R06 drag session → group transaction + persistence + no-open-after-drag in `flamewm-desktop-core` layout — CLOSED per verify PASS.
  - R07 label → multi-line bounded label in `flamewm-desktop-core` presentation — CLOSED per verify PASS.
  - R08 image slot → RGB8/revision projection via `flamewm-render-core` + `flamewm-ui-x11` bridge — CLOSED per verify PASS (prior `model.rs` attribution BLOCK superseded by verify PASS; no source mutation by this step).
  - R09 media visibility → playing/paused/stopped/available/unavailable matrix in `flamewm-shell-core` status facade — CLOSED per verify PASS.
- Protected contracts P01-P10: remain UNVERIFIED at ledger level pending read-only VERIFY job recording exact commands/exits; P11-P13 remain BLOCKED (no source-backed definition supplied).
- Residual items / unavailable proofs (explicitly NOT claimed):
  - `xdotool` missing, so grab proof unavailable.
  - Per-window Map State not captured.
  - No phase log markers.
  - Desktop `1920x1080` oversized vs `1350x641` noted.
- No runtime screenshot proof claimed beyond XEPHYR-CANARY mean `13115.9`; no final DONE claimed by this ledger step.

## 2026-09-07 Handoff Capsule (ledger only; no source change; no build/test run by this step)

- Source identity: version `0.0.7`, HEAD `bdc92a2a738c1278d243455f94e8494d62e450f7`, no remotes. Classification `DIRTY/FAILED_CONTINUATION`; live porcelain 97 entries (≈55 tracked modified incl. 7 deleted `flamewm-wm/*` sources, ≈40 untracked). Current dirty source wins; no stash/reset/clean.
- Last known green gates: NONE as live proof. Prior static PASS history (fmt/check/static/arch, ~243 tests, Xephyr canary) not re-executed here.
- R01-R09: per table above (all KEEP except R08 attribution BLOCK, now CLOSED per prior SHELL-UI-WAVE-VERIFY PASS mapping). R10-R14: handoff names only, no source-backed definitions — BLOCKED, no claim.
- P01-P10 UNVERIFIED; P11-P14 BLOCKED (no source-backed definitions).
- Waves J01-J11 + J12 plan: J01-J11 overlap dirty paths and stay BLOCKED under collision groups; J12 is read-only VERIFY (exact commands/exits) only.
- ROOT CAUSE (ledger-level): failed continuation left large dirty overlap with no ownership packets and no live verified gate; ledger PASS history cannot substitute for execution.

## 2026-09-07 Continuation Handoff v62 (ledger only; no source change; no build/test run by this step)

### Identity

- Continuation recovery handoff v62 (FlameWM 0.0.7, archive `1630_7_9_26.tar.gz`). Date `2026-09-07`. HEAD `bdc92a2a`, branch `main`, workspace version `0.0.7`.

### R01-R11 requirements (all UNVERIFIED)

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

### Protected contracts P01-P13 (all UNVERIFIED)

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

### Source state

- `DIRTY/FAILED_CONTINUATION`. Deleted `crates/flamewm-wm` superseded by untracked `crates/flamewm-wm-x11`; `docs/CANONICAL_OWNERS.md` still names the old path (SOURCE_DRIFT).
- `cargo clippy` NOT installed; `rustc 1.93.1`.

### READY frontier

- Wave 1: J01 (first-present shell `main.rs` + `runtime.rs`), J02 (xephyr `scripts/xephyr`), J03 (warning cleanup — convergence needed vs J01 on shell/`runtime.rs`), J04 (desktop interaction).
- Wave 2: renderer transparency (serialized J05). Wave 3: J06 calendar, J07 cursor, J08 desktop menus/selection, J09 shell context anchor. Wave 4: J10 window chrome. Wave 5: proof.

### Next gate (Wave 1 gate)

`tools/architecture_guard.py` + `cargo fmt --check` + `cargo check --workspace` + `cargo build --release -p flamewm-wm -p flamewm-desktop -p flamewm-shell` + `FLAMEWM_XEPHYR_CANARY=1 ./xephyr`.

No gate is claimed green in this execution — none has run. Preserve all dirty work; no stash/reset/clean/checkout/restore.
