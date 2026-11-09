# Starling desktop: competitor analysis

Source: `.vendor/starling-main/` (read-only reference; ideas only, no code or assets to ship).
Date: 2026-11-08. Method: static reading and grep. No build, no run. Screenshot viewed: `ui/img/desktop-2d.webp` (converted to PNG in scratchpad).

## 1. Licence

- Starling's own code: Apache-2.0 (`.vendor/starling-main/LICENSE`, `NOTICE`; SPDX headers in source). Porting an idea is fine; copying code needs attribution and the NOTICE.
- `sdk/` is a Swift port of Flutter and keeps Flutter's licence (`HANDOFF-winshell-perf-and-fixes.md`, "framework port"). The C++ engine is a Flutter fork (`starling-engine`, not in the tree).
- `.vendor/**` is read-only evidence in FlameWM (`AGENTS.md`). Use this file for ideas only.

## 2. Architecture and stack

- Language/UI: Swift (SwiftPM) on a Flutter-style widget framework (`sdk/`, "FlutterSwift"), rendered by the Flutter engine's C core. Browser builds use Swift WebAssembly + skwasm.
- Desktop: own Wayland compositor (`shell/Sources/WaylandServer/`, generated protocol bindings) plus an in-tree X11 server (`shell/Sources/DesktopShellApp/X11/`). DRM/KMS backend, RDP server, PipeWire screencast, portal service, IME bridge, notifications service (C).
- Terminal daemon: `termd/` (C, ~1.8k LOC `termd.c`, `protocol.h` 156 LOC). Sessions outlive clients.
- Apps: `apps/` (Terminal, Office Writer/Slides/Sheets, Files, Settings, etc.) each a SwiftPM package.
- Registry: `registry/catalog.d/*.app` is the single description of each app; `/var/lib/starling/installed.d/` is written by `app-install` and watched via inotify (`CLAUDE.md`, "Apps are data, not code").

Module map (Swift/C/TS/JS/etc. LOC, `loc-by-module` run, `find -exec cat | wc -l`):

| Path | LOC | Role |
|---|---|---|
| `sdk/` | 395,207 | Flutter→Swift framework port, widgets, platform hosts |
| `apps/` | 118,418 | First-party apps |
| `shell/` | 87,830 | Desktop shell, compositor, WM, dock, 3D view |
| `macos-compat/` | 11,270 | Research: Mach-O on Linux |
| `host/` | 8,103 | Windowed host plumbing |
| `build/` | 7,215 | Stage/package/run scripts |
| `termd/` | 4,339 | Session daemon |
| `record/` | 2,743 | Recording |
| `registry/` | 1,633 | App registry |
| `power/`, `network/`, `ui/`, `audio/`, `time/` | 570 / 900 / 499 / 297 / 207 | Small services |

Largest shell files (`shell/` Swift total 38,381 LOC): `DesktopShell.swift` 9,275; `Desktop3D.swift` 3,154; `WaylandIntegration.swift` 2,489; `WindowManager.swift` 1,240; `LinuxProcessAppManager.swift` 1,214; `main.swift` 1,155; `AgentBroker.swift` 1,062; `WorkspaceSpace.swift` 1,032.

Read against FlameWM: the Starling shell is one 9k-line widget tree. FlameWM's split (pure `flamewm-shell-core` model, `flamewm-ui-core`, typed owners) is cleaner. Do not copy the monolith.

## 3. Feature matrix vs FlameWM (non-dock)

Legend: have = both; missing = FlameWM lacks; better = whose version is stronger for FlameWM's goals.

| Feature | Starling | FlameWM | Verdict |
|---|---|---|---|
| Compositor / WM core | Own Wayland + X11 compositor, DRM | X11 + Wayland WM (`flamewm-wm-*`) | have, FlameWM better on WM authority separation |
| Session-surviving terminal | termd, byte-offset reattach (`termd/README.md`) | terminal crate present, persistence not verified | missing (see §4 item 1) |
| Agent broker | Default-deny, audited, owner-scoped (`AgentBroker.swift:1-30`) | `flamewm-api` exists; no agent broker found | missing (item 2) |
| 3D city desktop | `Desktop3D.swift` 3,154 LOC | none | REJECT (out of WM scope, high cost) |
| Open/minimise zoom animations | `OpenAnimations.swift` (easeInOutCubic, scale 0.88→1) | none; DOCK_DESIGN §3 says no animation | REJECT for now (conflicts with current policy) |
| Multi-style shell (macOS/Fluent) | `ShellStyle.swift`, one file per style | skin tokens (`flamewm-skin`, `ui/shell/flamewm.css`) | have; adopt the "one file per style" rule (process item 4) |
| App catalog as data | `registry/catalog.d/*.app` + inotify | desktop entries (not verified) | ADAPT (item 3) |
| Settings persistence | RDP choice, idle timeout restored at startup (`DesktopShell.swift:1259-1275`) | settings crates (`flamewm-settings`, `flamewm-platform/settings.rs`) | have; FlameWM more typed |
| Session lock | not found in grep | `flamewm-lock` | better (FlameWM) |
| Screencast/recording | `RecordingService.swift` 803, PipeWire | not verified | REJECT (out of scope) |
| RDP server | `RdpServer` C + Swift | none | REJECT (out of scope) |
| Portal / file dialogs | `PortalService` | none verified | REJECT for now |
| Notifications daemon | `NotificationService` C | `flamewm-shell` notification UI | have |
| Dock | macOS-style pill, magnify, bounce, indicators | `flamewm-shell-core/src/dock.rs` pure model; surface not built | see §5 |
| Crash watchdog / auto-restart of shell | not found (only comments in `main.swift:254,935`) | `.agents/RECOVERY_TODO.md`, `REGRESSION_LEDGER.md` (process, not runtime) | Nothing to port |

Summary: Starling is broader (own compositor, RDP, terminal daemon, 3D view). FlameWM is narrower and more disciplined about WM authority. The ideas worth taking are small and concrete.

## 4. Ranked candidate features (non-dock, max 8)

Costs are estimates from reading code, not measured.

1. **Session-surviving PTY with byte-offset reattach** (`termd/termd.c`, `termd/README.md`). ADAPT. Cost: one ring buffer per session (memory = buffer cap × sessions), one reader thread per pty. Files: `crates/flamewm-terminal/src/app/*` (session layer), `crates/flamewm-reactor` for fd sources. Note: Starling's protocol test (`termd/test-termd.py`, 27 checks) is a good pattern.
2. **Default-deny agent broker** (`shell/Sources/DesktopShellApp/Shell/AgentBroker.swift:1-30`): agents see only windows they launched, every effectful call is audited, versioned JSON-lines hello. ADAPT only if FlameWM wants agent control. Cost: a unix socket plus an audit log. Files: `crates/flamewm-api/src/lib.rs`; socket source must go through `flamewm-reactor`.
3. **Apps as data, inotify on install dir** (`CLAUDE.md` "Apps are data, not code"). ADAPT. Cost: one inotify watch, negligible CPU. Files: `crates/flamewm-integrations-linux/src/icon_service.rs` area, desktop-entry loader.
4. **Style = one file + one registry entry, "no `if style ==` anywhere else"** (`CLAUDE.md` Standing directions; `ShellStyle.swift`). ADOPT as a rule. Cost: none. Files: `ui/shell/flamewm.css` tokens, `crates/flamewm-skin/src/shell_tones.rs`.
5. **Open/minimise zoom from dock icon** (`OpenAnimations.swift`). REJECT now. Conflicts with DOCK_DESIGN §3 (no animation). Revisit only with a user decision.
6. **Pinned-app ordering with drag-reorder and sliding gap** (`DesktopShell.swift` `_dockDragActive`, `_dockGapAfter` ~7186). ADAPT the reorder logic (already planned as `move_pinned`). REJECT the sliding animation unless animations are approved.
7. **Live window thumbnails on hover (Fluent taskbar)** (`DesktopShell.swift:3436-3447`). REJECT for now. Cost high: one capture per hovered window, GPU textures, and FlameWM's X11 path has no equivalent.
8. **Per-user settings restored at startup with explicit env override** (`DesktopShell.swift:1259-1262`). ADOPT the pattern; FlameWM's `dock.*` spec already covers it (`docs/DOCK_SETTINGS_SPEC.md`).

## 5. Dock: deep dive

### 5.1 Source files

- Starling: `shell/Sources/DesktopShellApp/Shell/DesktopShell.swift` (`macosDock` 3196; `_dockMetrics` 7211; `_buildDock`/pill ~7600–7700; `_buildDockIcon` ~8050–8110; `_buildDockIconMenu` 8259); `Shell/OpenAnimations.swift` (`DockBounce` 192–260); `Utils/DesktopTheme.swift` (`kDock*` constants 23–90, colour roles 137–142, 231–236); `Utils/ShellStyle.swift` (macOS/Fluent colour mapping 440–445; metrics 87–89).
- Starling sdk example: `sdk/Examples/WinShellBar/Dock.swift`, `DockBloc.swift` (not used by the shell).
- FlameWM: `crates/flamewm-shell-core/src/dock.rs` (318 LOC; `merge_dock`, `pin_app`, `menu_items` 152, `slot_at`, `drag_off_removes`, `gradient_background`); `crates/flamewm-shell-core/src/autohide.rs` (`HIDE_DELAY` 400 ms, `SLIDE` 200 ms, `FRAME` 16 ms); `ui/shell/dock.html` (24 slots); `ui/shell/flamewm.css:326-338` (dock tokens); `docs/DOCK_DESIGN.md`; `docs/DOCK_SETTINGS_SPEC.md`.

### 5.2 Metrics (Starling, from `DesktopTheme.swift`)

| Item | Starling value | FlameWM value |
|---|---|---|
| Icon size | 56 pt (`kDockIconSize`) | 44 px default (`dock.iconSize` 24–96) |
| Icon padding (gap between icons) | 6 pt; 12 pt after launcher slot (`_dockGapAfter` 7186) | 3 px gap (DOCK_DESIGN §5) |
| Outer padding | 14 pt horizontal (`kDockHorizontalPadding`) | 6 px (DOCK_DESIGN §5) |
| Icon corner radius | 12 pt (`kDockIconCornerRadius`) | not specified |
| Pill corner radius | 32 pt (`kDockCornerRadius`, full pill) | `--radius-xl` token |
| Pill height | `kDockHeight` = `bottomBarHeight` 76 pt (`ShellStyle.swift:87`) | bar height not specified (icon 44 + 2×6 pad) |
| Bottom margin | 6 pt (`bottomBarMargin`) | 8 px (DOCK_DESIGN §5) |
| Border | 1 pt hairline `dockRim` (`DesktopShell.swift` pill, `DesktopTheme.swift:233` 0x59FFFFFF) | 1 px `--rim` |
| Fill | vertical gradient `dockGradientTop` 0x12FFFFFF → `dockGradientBottom` 0x05FFFFFF (dark); light theme 0xB3FCFCFC → 0x8CF2F2F4 | `--panel` solid (`#000000bf` in `:root`) |
| Blur | `BackdropFilter` blurSigma 3.5, saturation 1.12 (`_dockGlassFilter`, line ~7591). Unused constant `kDockBlurSigma` 24 is dead code (`DesktopTheme.swift:89`) | none |
| Shadow | offset (0,6), blur 24, `dockShadow` 0x40000000 | none |
| Magnify max scale | 1.5 (`kDockMagnifyMaxScale`) | none (rejected) |
| Magnify radius | 150 px cosine falloff `1 + 0.5·(1+cos(π·d/r))·boost` (`_dockMetrics` 7230–7240) | none |
| Running dot | 4 pt, `dockIndicator` 0xF2FFFFFF, rim 0x59000000 0.5 pt, shadow blur 1.5 (`DesktopShell.swift` ~8060–8070) | 2 px radius, `--accent` (`.dock-dot`, flamewm.css:332) |
| Dot strip | 10 pt inset under icon | not specified |
| Separator | not found in Starling dock code | 1 px `--rim` (`.dock-separator`, flamewm.css:333) |

### 5.3 Animation (Starling)

| Animation | Curve | Duration | Notes |
|---|---|---|---|
| Hover magnify | continuous cosine on pointer X, no tween | per pointer move | full dock rebuild (`_updateDockHover`) |
| Launch bounce (`DockBounce`) | sine hop, 22 pt high, first 70% of period, then rest | 700 ms, repeats while launch pending | `OpenAnimations.swift:197-260`; active while `_pendingAppLaunches` (9136–9210) |
| Open zoom | `easeInOutCubic`, scale 0.88→1.0 | not extracted | `OpenAnimations.swift:79,179` |
| Minimise zoom | `easeIn` close scale | not extracted | `OpenAnimations.swift:122` |
| Slot shift on drag | `AnimatedContainer` | `animDuration` | DesktopShell ~7390–7400 |

### 5.4 Interaction

- Click: launch or focus (`_launchFromLauncher`, pending set). Right-click: menu anchored at icon X (`_dockMenuAppId`, `_dockMenuAnchorX`, 396–400; open 3393).
- Context menu items (`_buildDockIconMenu`, 8259): "Keep in Dock" / "Remove from Dock", "Open", "Quit" (plus "Show" seen in strings). Model is equivalent to FlameWM's `menu_items` (dock.rs:152).
- Drag reorder: `_dockDragActive`, `_dockDragIndex`, `targetIdx`; neighbours slide via `AnimatedContainer`. Drag-off removal: not verified.
- Hover label: app-name bubble above icon, baseline `kDockLabelBottom` = 10 + 84 + 8 (`DesktopTheme.swift:52-53`).
- Auto-hide: not found in Starling. FlameWM has it (autohide.rs). FlameWM is better here.
- Settings: no dock-specific settings UI found in Starling grep (not exhaustively checked).
- Previews: no dock hover previews. Live thumbnails exist only in the Fluent taskbar (`DesktopShell.swift:3436-3447`).

### 5.5 Visual comparison (from `desktop-2d.png`, 1920×1080, viewed)

Starling's dock is a dark, rounded, translucent pill centred at the bottom edge, about 440 px wide, holding about 8 icons (launcher grid, settings, files, terminal, notes, calculator, media, App Center). Icons are full-colour tiles with glyphs, sitting on a thin light rim. No visible running-dot in that frame. The pill reads as glass over the wallpaper. FlameWM's dock design (DOCK_DESIGN §5) is a flatter, solid `--panel` pill with a 1 px rim, 44 px icons, a 2 px accent dot, and no magnification. FlameWM's look is quieter and cheaper. Starling's look is richer and costlier.

I did not view the 3D screenshot or slide PNGs (`ui/desktop/slides/*.png`).

### 5.6 Dock verdicts

Ranked, max 8.

1. **Running dot + 1 px rim + token colours** (`dockIndicator`, `dockIndicatorRim`). ADOPT as tokens only, using existing `--accent`/`--rim`. Cost: none. Files: `ui/shell/flamewm.css:332`, `ui/shell/dock.html`.
2. **Pinned/running separator and merged list** (`merge_dock`). ADOPT, already built in FlameWM. Starling has no separator. Cost: none.
3. **Right-click menu set** (Keep/Remove, Open, Close windows). ADOPT, already modeled (`dock.rs:142-167`). Files: `ui/shell/dock-menu.html`, `flamewm.css:335-338`.
4. **Hover name label** (`kDockLabelBottom` bubble). ADOPT with the existing tooltip path, as DOCK_DESIGN §2 plans. Cost: one tooltip node, no per-frame work.
5. **Launch bounce** (700 ms, 22 px sine hop, repeat while pending). ADAPT. Only with explicit user approval, because DOCK_DESIGN §3 says no animation. If approved, use CSS `transform` only, stop on first frame, and tie to `ui/shell/launch-feedback.html`. Cost: one timer at 16 ms for the bounce duration only.
6. **Hover magnification** (cosine falloff, 1.5× max, 150 px radius). ADAPT as an opt-in setting, default off. Use `transform: scale()` on the icon, not layout, to avoid a full dock rebuild per pointer move. Cost: per-move style recalc on up to 24 slots. Files: `ui/shell/dock.html`, `crates/flamewm-shell-core/src/dock.rs` (pure scale function), `docs/DOCK_SETTINGS_SPEC.md`.
7. **Top-lit gradient fill and drop shadow** (`dockGradientTop`/`Bottom`, shadow 0,6,24). ADAPT the gradient via existing `gradient_background` (dock.rs:171). REJECT the shadow unless the GPU path is confirmed; cost is a blur per frame. Verify against `docs/GPU_BACKENDS.md` (not read).
8. **Glass blur (BackdropFilter sigma 3.5)**. REJECT on the X11 path. Blur per frame is the most expensive item here. Revisit only with the GPU backend.

Not adopted: live dock previews (no Starling dock equivalent), 56 pt default icon size (FlameWM's 44 px is smaller and specified).

## 6. Agent-driven development: how Starling is organised

Evidence: `CLAUDE.md` (33.7 KB, root), `shell/Sources/DesktopShellApp/CLAUDE.md`, `sdk/CLAUDE.md`, `HANDOFF-*.md` (3 files), `docs/plans/*.md` (~35 design notes), `docs/perf/*` (dated benchmarks), `docs/issues/`, `sdk/tools/run-tests.sh`, `termd/test-termd.py`, `termd/test-ssh-attach.py`, `build/stage.sh`, `bootstrap.sh`.

Sections of root `CLAUDE.md`: Layout (33), Build & iterate (67), Apps are data not code (250), Standing directions (295), Traps that have cost real time (336).

Techniques:
- **Scoped guidance**: root plus per-subtree `CLAUDE.md` (shell, sdk). Each carries its own rules.
- **Hard device-safety rules** (`CLAUDE.md:112-150`): "This dev box has a real display. Never force a connector on it." Concrete forbidden commands, verify-before-acting steps, and a revert rule.
- **Single definition of layout** (`build/stage.sh`): "change assembly there, never in" other scripts.
- **Dated trap log**: "Traps that have cost real time" with the failure and the fix.
- **Handoffs**: dated checkpoint files with a headline, a "verified" table of commits and proofs, and a first action at top ("the box is DOWN ... first thing to deal with").
- **Design notes in `docs/plans/`** and **benchmarks in `docs/perf/`** with dated folders and JSON + MD pairs.
- **Protocol test as the spec**: `test-termd.py` runs the same checks over unix socket and stdio; `test-ssh-attach.py` covers the transport that the protocol test cannot reach.
- **Memory**: handoffs name "Memory updated this session" entries.

## 7. Process lessons FlameWM lacks (ranked, max 5)

Note on the brief: it named `docs/AGENT_PLAYBOOK.md` and `scripts/gate`. Neither exists (`ls` failed). FlameWM has `scripts/lint`, `scripts/check`, `scripts/release-gate`, `scripts/verify-visual`, `scripts/screenshot-acceptance`, and `.agents/REGRESSION_LEDGER.md` (129 lines), `.agents/RECOVERY_TODO.md` (190 lines), `.opencode/ROUTES.md`, `.opencode/HARNESS.md`.

1. **Host-safety guard rails for live hardware.** Starling forbids forcing connectors on the dev display and requires an in-session revert (`CLAUDE.md:112-150`). FlameWM's `.opencode/HARNESS.md` has no equivalent. ADOPT as a short rule set in `.opencode/HARNESS.md`. Cost: none.
2. **Dated "Traps that have cost real time" section, linked from routes.** FlameWM's `.agents/REGRESSION_LEDGER.md` is similar in spirit but not linked from `ROUTES.md` as a must-read before a route. ADAPT: add a route row "any shell/input/DRM change → read ledger traps". Cost: none.
3. **Single definition of artifact layout with a "never re-inline" rule** (`build/stage.sh`, `CLAUDE.md:195-200`). FlameWM's packaging (`scripts/package`, `scripts/build-deb`) is probably split. Verify before acting. ADAPT.
4. **Style/feature registration as one file plus one list, with a grep-able anti-pattern** (`CLAUDE.md:295-300`). FlameWM can adopt it for skin/style tokens and dock variants. ADOPT as a rule in `AGENTS.md` or `ROUTES.md`.
5. **Protocol/transport parity tests** (`termd/test-termd.py --stdio`, `test-ssh-attach.py`). The lesson: pipe tests passed while the real ssh path was broken. FlameWM should require a real-transport check before claiming an IPC path works (`.opencode/ROUTES.md` runtime-evidence row). ADAPT.

## 8. Risks and limits

- Read-only, static. No runtime claim is made here. All behaviour above is from source; screenshot is a vendor image, not a FlameWM run.
- Some Starling files were not read in full (`Desktop3D.swift`, `WaylandIntegration.swift`, per-slide PNGs).
- Starling's own CLAUDE.md states "No security hardening on the app runtime" and "Wayland only", which are its choices, not FlameWM's.
- Licence: porting ideas is fine. Copying Swift code needs Apache-2.0 attribution; the Flutter-derived `sdk/` keeps Flutter's licence.
- Not checked: whether `.opencode/skills` already has a dock or style rule; whether `docs/GPU_BACKENDS.md` allows blur.
