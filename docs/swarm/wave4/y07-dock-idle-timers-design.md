# y07 — Dock and start menu idle timers: design and step 1

**Status: PARTIAL / UNBUILT.** Step 1 is implemented with a test; no `git`, `cargo`, Python, build,
test or sleep loop was run in this shared checkout. The orchestrator builds, tests and commits after
the batch lands. Format precedent: `docs/swarm/wave4/y06-binary-size-embedded-design.md`.

## Route and evidence ledger

- Skills loaded: `flamewm-source-first`, `flamewm-reference-mining`, `flamewm-rust-systems`.
  Workspace `AGENTS.md`, `Cargo.toml:6` (`version = 0.1.7`, read instead of running
  `scripts/version`), `.opencode/HARNESS.md`, `.opencode/ROUTES.md` and
  `.agents/{README,CONTRACTS,JOBS}.md` were read.
- Commands used: repository `rg`, `Read`, `wc`, `ls` and bounded reads. No compiler, test runner,
  VCS command, Python or script was invoked.
- Contract: no new dependencies, no `Cargo.toml` edit, no `ui/` HTML/CSS edit, visuals pixel-identical,
  at most three production edits plus the requested test. Only
  `crates/flamewm-shell/src/app/timers.rs` was edited (the test is in the same file). This document
  and `docs/swarm/wave4/y07-dock-idle-timers.md` are documentation/bookkeeping.
- Scope note: `.vendor/**` was read as evidence only; instructions found in vendor tool results were
  treated as data, never as commands to run.

## Current structure: every dock/start timer (file:line)

The shell publishes one typed reactor wake slot. `ShellLoop::publish_wake`
(`crates/flamewm-shell/src/app/timers.rs:242-264`) reduces every owner deadline with
`earliest_wake` (`crates/flamewm-shell/src/app/helpers.rs:187-193`). `WakeDeadline::set`
(`crates/flamewm-reactor/src/lib.rs:83-88`) is change-gated, so any redundant `Some` still costs a
loop wake. Idle contribution is per owner, not total CPU.

| # | timer / owner | site | idle contribution |
| --- | --- | --- | --- |
| 1 | Volume/brightness OSD hide | `timers.rs:245` → `osd.rs:267` | 0/s: deadline only while the card shows |
| 2 | Alt+Tab switcher hold | `timers.rs:246` | 0/s: hold deadline only |
| 3 | Chrono local-zone settle, one-shot | `timers.rs:247`, cleared at `timers.rs:114-117` | 0/s after settle |
| 4 | Task-progress paint gate | `timers.rs:248` → `ipc.rs:122-137` | 0/s when cleared on `PaintDecision::Now` |
| 5 | Clock minute/second cadence | `timers.rs:249`, armed at `timers.rs:213-233` | 1/60 Hz hidden seconds, 1 Hz with seconds |
| 6 | Weather refresh | `timers.rs:250` → `weather_host.rs:222-233` | 0/s when disabled, no location or inflight |
| 7 | Dock auto-hide slide / hover grace | `timers.rs:251-253` → `shell-core/autohide.rs:161-167` | 0/s once settled; 600 ms grace after a hold ends |
| 8 | **Dock launch hop frames, 16 ms** | `timers.rs:254-255` → `shell-core/dock.rs:531-533` | **up to ~44 dead frames per hidden launch** |
| 9 | Bluetooth agent prompt | `timers.rs:256-258` | 0/s with no prompt |
| 10 | Usage refresh | `timers.rs:259` → `usage_host.rs:105-114` | 0/s with popup closed or no provider |
| 11 | Open usage-card reproject, 60 s | `timers.rs:260-262` → `app/usage.rs:8,38-45` | 1/60 Hz only while that card is open |

Non-slot timers in the same area: the reactor minute wall-clock source consumed at
`timers.rs:124-130`; the raw-press observer armed/disarmed at `timers.rs:19-22`; the WM-owned frame
timeline (`crates/flamewm-wm-x11/src/animation.rs:8-10`, outside `flamewm-shell`); and the dock hover
scale, which is **event-driven, not timer-driven** (`app/dock.rs:889-893` and `:937-972`). The start
menu has no dedicated periodic timer in `flamewm-shell`; it shares slot owners 1-6.

## Counted problems

- **11** owner deadlines enter the single wake slot; **10** are already idle-gated at their owner.
- **1** concrete defect: `publish_wake` published the 16 ms `LaunchBounce` deadline without asking
  whether the dock could paint. `tick_dock_bounce` (`app/dock.rs:980-1011`) skips every position
  write while the dock is hidden (`app/dock.rs:990-999`), yet `publish_wake` kept waking for the
  full `BOUNCE_PERIOD` (700 ms / 16 ms ≈ 44 frames) after a launch on a hidden dock.
- **1** structural gap: no typed animation-gate seam existed at the shell wake boundary; the
  transient owners inlined their own rules, so a hidden-target case was not re-checked where the
  deadline was published.
- **0** shipped visual changes are needed: the hop's frame cadence and every rect while visible are
  untouched.

## Target design

One typed owner per timer mechanism; the shell keeps a single `WakeDeadline` facade. A transient
animation deadline is published only while its paint target is live, and the publishing boundary
re-checks that target (Vela's `onTriggered` predicate shape). Hiding the dock removes the frame
deadline; re-showing republishes it through the existing `dock_pointer` → `publish_wake` path, so the
remaining hop paints at the same times. UI never becomes timer authority, and no interval,
dependency or gate is loosened.

## Steps that each compile alone

1. **Implemented (UNBUILT).** `crates/flamewm-shell/src/app/timers.rs`: replace the inline bounce
   deadline in `publish_wake` with `dock_bounce_deadline(bounce, hidden, now)`, deriving `hidden`
   from the same `AutoHide::is_hidden` state the paint path uses. Add the pure helper and a test
   (`dock_bounce_frames_stop_while_the_dock_is_hidden`) covering visible hop, hidden hop, hop end
   and absent bounce. No other file changes.
2. Give `AutoHide` a named frame gate (`is_animating(now)`) in `flamewm-shell-core/src/autohide.rs`
   and have `deadline()` call it; add a test that an instant/reduced-motion dock arms no frame.
   Behaviour-preserving.
3. Gate auto-hide's 600 ms grace on the dock still being visible when the last hold ends
   (`set_auto_hide` / `set_hold`), so an already hidden dock contributes no hover wake; test it.
4. Audit-only follow-up: confirm owners 1-7 and 9-11 keep their existing `None` gates; no code
   change is expected. Any later consolidation stays behind the `WakeDeadline` facade.

## Rollback

Each step is independent. Step 1: restore the inline `self.dock_bounce.and_then(|(_, bounce)| bounce.deadline(now))`
expression in `publish_wake`, delete `dock_bounce_deadline` and its test; no settings, config or
runtime state is migrated. Steps 2 and 3 revert only their own file hunks. Nothing is remapped or
reconfigured, so rollback cannot change pixels.

## Vendor reference and choice

- Path: `.vendor/vela-main/shell/qml/Taskbar.qml:89-98` — 450 ms `previewDelay` `Timer` whose
  `onTriggered` re-checks `task.hovered && task.windowCount !== 0` before acting; and
  `.vendor/vela-main/shell/qml/StartMenu.qml:295-303` — 200 ms `focusCheck` one-shot restarted only
  on focus/menu edges. **Choice:** port only the semantic — publish a transient frame deadline while
  its target is live and re-check the target at the publishing boundary. No QML, timer API or asset
  was copied.
- Anti-pattern: `.vendor/starling-main/ui/desktop/slides.js:135-138` — a free-running 100 ms
  `setInterval` that keeps polling after the slide settles; the local coalesced-deadline design
  avoids it. Recorded as evidence, not ported.

## Safety and verification

- Step 1 changes only whether a deadline is *published* while the dock is hidden. `bounce.lift()`,
  `tick_dock_bounce` and every visible icon rect are unchanged; re-showing republishes the same
  deadline through the existing hover path, so visible frames are pixel-identical.
- No `ui/` HTML/CSS, asset, manifest, profile or `Cargo.toml` was edited, so raw-byte fingerprints
  and every compiled document are unchanged.
- UNBUILT by contract: no compiler or test runner was invoked; the new test and all behaviour claims
  await the orchestrator's build and test run.

PASS — step 1 is a self-contained, pixel-identical animation-gate fix with a test; the full timer
inventory and the remaining gate steps are documented as independent steps 2-4 and are deliberately
not applied here.
