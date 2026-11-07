# Pair brief: Claude (orchestrator) + opencode GPT 6.1 Terra

Date: 2026-11-07. Repo: `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender`, branch `main`.
Read first: `AGENTS.md`, `.opencode/HARNESS.md`, `.opencode/ROUTES.md`, `docs/ARCHITECTURE.md`,
`docs/PLAN_2026-11-07_SCOPED_SWARM.md` (Results sections = what is already done).

## 0. How we avoid colliding

Two agents edit the same working tree. The rule is **disjoint path ownership**.

**Claude owns (do NOT edit, even for "tiny" fixes; message the user instead):**
- `crates/flamewm-wm-x11/**`, `crates/flamewm-wm-wayland/**`, `crates/flamewm-wm/**`
- `crates/flamewm-shell/**`, `flamewm-shell-core/**`, `flamewm-desktop/**`, `flamewm-desktop-core/**`
- `crates/flamewm-api/**`, `flamewm-control-wire/**`, `flamewm-control-core/**`, `flamewm-platform/**`
- `crates/flamewm-integrations-core/**`, `flamewm-integrations-linux/**`, `flamewm-clipboard/**`, `flamewm-tray/**`
- `crates/flamewm-render-x11/**`, `flamewm-render-core/**`, `flamewm-render-compiler/**`, `flamewm-ui-document/**`
- `crates/flamewm-image-core/**`, `flamewm-profiler/**`, `flamewm-skin/**`, `flamewm-window-core/**`
- `ui/shell/**`, `ui/flamewm.css`, `ui/index.html`
- root `Cargo.toml`, `Cargo.lock` (see section 3), `.cargo/**`, `tools/architecture_guard.py`, `docs/ARCHITECTURE_EXCEPTIONS.toml`
- `docs/PLAN_2026-11-07_SCOPED_SWARM.md`

**Terra owns (Claude will not touch):**
- `crates/flamewm-settings/**`, `flamewm-settings-core/**`, `ui/settings.html`, `ui/settings.css`, `ui/settings-accessibility.css`
- `crates/flamewm-screenshot/**`
- `crates/flamewm-explorer/**`, `flamewm-finder/**`, `flamewm-terminal/**`, `flamewm-imageviewer/**`,
  `flamewm-archiver*/**`, `flamewm-task-manager/**`, `flamewm-assistant*/**`, `flamewm-weather/**`, `flamewm-ribbons/**`
- matching UI dirs: `ui/explorer`, `ui/finder`, `ui/terminal`, `ui/imageviewer`, `ui/archiver`, `ui/task-manager`, `ui/assistant`
- `scripts/*acceptance*`, `scripts/tests/**`, `tools/tests/**`, your own new doc `docs/TERRA_RESULTS.md`

If a goal needs a Claude-owned file, add a one-line request to `docs/TERRA_RESULTS.md`
under "Requests for Claude" and keep going on the rest. Do not edit it.

## 1. What Claude is doing now (so you know what is moving)

Five workers are live on: lazy session-bus integrations for the X11 WM; `attention` and `skip_taskbar`
window-snapshot fields (api, wire, shell task list); Wayland scale-aware layout and DRM output scale;
wallpaper decode-cache pruning, desktop cursor-retry spam, flaky-test fixes; quick-popup open latency.
Then Claude keeps working the remaining list: DRM runtime checks, multi-output, xdg-activation/foreign-toplevel/screencopy,
file splits, compile time. The shared tree may fail to compile in Claude-owned crates while those land.
If your build breaks inside a Claude-owned crate, wait 5 min and retry; do not fix it.
Build with an isolated target dir to avoid lock contention and stale artifacts:
`export CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-terra CARGO_BUILD_JOBS=3 RUST_TEST_THREADS=2`.
Host load is often >12. Prefer `cargo test -p <crate>` and `cargo check -p <crate>` over workspace builds while
iterating; run the workspace test once before each push.

## 2. Goals (big features, in priority order; each = one commit series)

### G1. Settings: Displays page (end to end)
Resolution, refresh rate, scale (incl. fractional 1.0 to 2.0 in 0.25 steps), arrangement, primary, orientation,
with a 15 s auto-revert confirmation dialog. Use the existing display transaction service in
`flamewm-settings-core`/`flamewm-platform` (read-only for platform; if it lacks an API, request it).
Persist via existing settings keys (`scale.wayland-0` is read by the Wayland WM; write that key for the Wayland scale).
Deliver: UI in `ui/settings.html`/`.css` using existing tokens only (no new raw colours; new colours need a `:root`
token plus `shell_tones.rs` entry, which is Claude-owned, so request it), core logic with unit tests, acceptance
script `scripts/settings-acceptance` extended.

### G2. Settings: Night Light + Accessibility pages
Night light: manual and sunset/sunrise schedule, colour temperature 1000 to 6500 K, preview toggle; store in
settings; expose through `flamewm-settings-core` types (the backend that applies gamma is Claude's later task:
write the settings contract and document it in `docs/TERRA_RESULTS.md`).
Accessibility: high contrast, reduced motion, text scale (0.8 to 2.0), cursor size, sticky keys, slow keys.
`ui/settings-accessibility.css` already exists; wire the page, persist keys, unit tests for validation and migration.

### G3. Screenshot and screen-region tool (crate `flamewm-screenshot`)
Full screen, window, and region capture with an interactive region selector overlay, copy to clipboard
(use `flamewm-clipboard` public API read-only), save to `~/Pictures/Screenshots` with templated names,
delay timer, and a post-capture notification via the existing notification D-Bus spec. X11 path first (XGetImage / SHM);
design the capture trait so a Wayland screencopy backend can be added later. Tests for naming, region math,
encode paths. Measure capture of 2560x1440 (time, peak RSS) and report numbers.

### G4. Explorer upgrade
Review `crates/flamewm-explorer` for the biggest gaps vs Dolphin/Nautilus: background directory loading
(never block the UI thread), thumbnail cache with bounded size, multi-select + drag/drop, rename-in-place,
undo for move/delete (trash), search within folder, breadcrumb path bar. Pick the top 4 by value, implement with tests,
keep the existing visual design. Measure directory-open latency for a 10k-entry folder before/after.

### G5. Quality sweep inside your own area
For every crate you own: zero rustc warnings, files <700 lines (split by responsibility, pure moves, keep public paths),
dedupe helpers across your crates into the nearest shared one you own (or ask for a Claude-owned shared home),
repeated log lines rate-limited, no work on idle timers <50 ms. Report line counts before/after.

## 3. Git protocol (important: shared tree, shared branch)

1. Commit **only your paths**: `git add <your paths>` then `git commit -m "<type>(<area>): <msg>"`. Never `git add -A` or `git commit -a`
   (that would sweep in Claude's in-progress work).
2. Before each push: `git pull --rebase --autostash origin main`, then `git push`. If the rebase conflicts in a file you do not own,
   abort and tell the user.
3. Never touch `Cargo.lock` directly. If your changes alter it, commit it only when `git diff --stat Cargo.lock` shows lines
   solely from your crates; otherwise leave it and note it in `docs/TERRA_RESULTS.md`.
4. Gate before every push (all must pass; report real exit codes):
   - `cargo test -p <each crate you touched>` exit 0
   - `cargo check --workspace` exit 0 with zero `^warning` lines
   - `python3 tools/architecture_guard.py` exit 0 (do not edit the guard or exceptions file; request changes)
   - no new `.vendor/**` use; new dependencies need an explicit entry request
5. Commit trailer: end messages with your own co-author attribution line.
6. Never force-push. Never rewrite history. Never delete branches.

## 4. Engineering rules (same as the rest of the repo)

- Current source and runtime evidence outrank docs. Runtime claims need fresh executed evidence (run it, show output).
- UI design must not change visually unless the goal says so. No raw colours outside `:root`.
- Visual checks: isolated Xvfb on a free display (`pgrep -a Xvfb` first), screenshot with `import -window root`, run from a temp cwd,
  kill only PIDs you started. Never `pkill -f` broadly. Never attach to the user's real display or `wayland-0`.
- No new always-on threads; use event-loop sources or short-lived pools. Bounded caches. No per-frame allocation in hot paths.
- Failures: report symptom, cause, fix. After three failed fixes, revisit the assumption.

## 5. Reporting

Keep `docs/TERRA_RESULTS.md` updated with a section per goal: files changed, commands with exit codes, measured numbers,
what was not verified, one-line risk, and a "Requests for Claude" list. Claude reads it after each batch and picks up requests.

## 6. Suggested todo order

1. Read the docs above; confirm build in your target dir (`cargo check -p flamewm-settings`).
2. G1 (Displays) core types and tests, then UI, then acceptance script; commit and push.
3. G2 (Night Light + Accessibility); commit and push.
4. G3 (Screenshot); commit and push.
5. G4 (Explorer); commit and push.
6. G5 sweep; final workspace gate; update `docs/TERRA_RESULTS.md`.
