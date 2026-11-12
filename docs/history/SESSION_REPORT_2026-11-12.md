# Orchestration session report, 2026-11-12

Status snapshot: written 2026-11-12 while the session is still running. Four
workers are in flight (polkit card, per-output scale, X11 magnifier, UiDocument
attribute setter). Anything not listed under "Landed" is NOT on origin/main.

## 1. Mandate and method

- Mandate: the `/goal` loop in `docs/GOAL_LOOP.md` plus the permanent
  reference-first rule in the user's `~/.claude/AGENTS.md`: never ask questions,
  never park work as "decision needed", resolve doubts from `.vendor/**`
  (read-only) and standards (freedesktop, XDG, WAI-ARIA, polkit), cite the
  precedent in the commit, push each verified slice.
- Roles: the primary session is orchestrator only. It scopes briefs, verifies
  landings and writes docs(backlog) reconcile commits. Every code slice goes to
  a `general` CODER subagent in a private worktree `/tmp/opencode/wt-<slug>`
  cut from origin/main. The main checkout (many unrelated uncommitted files) is
  never touched, stashed, reset or committed.
- Worker gate: `scripts/gate <crate>` with `FLAMEWM_TOOLCHAIN_BIN`,
  `CARGO_HOME=/tmp/opencode/<home>`, `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`,
  `GATE_LOG`. Every `EXIT_` line (guard, fmt, clippy, test) must be 0. Then
  fetch, rebase, re-gate if the crate closure changed, commit with the
  `Co-Authored-By: Claude Sonnet 5.5` trailer, `git push origin HEAD:main`
  fast-forward only.
- Verification of each landing by the orchestrator: `git merge-base
  --is-ancestor <sha> origin/main`, commit trailer, no conflict markers
  (`git grep -l '^<<<<<<<'`), EXIT lines read from the gate log (not the
  wrapper echo), `git show --stat`.
- Safety rules: workers never probe host system buses (one read-only
  `CheckAuthorization` once made the host polkitd abort, status 6/ABRT); they use
  private `dbus-daemon` mock buses, private Xvfb and nested Wayland sessions.
  Disk stop limits: 20 GB on the repo filesystem, 10 GB on /tmp.
- Concurrency: gates share one cargo lock
  (`/tmp/opencode/flamewm-terra-cargo.lock`), so workers run in parallel but
  gates and landings are serial and often queue 30 to 60+ minutes. Each landing
  that changes a shared crate closure forces queued gates to rebase and re-gate.

## 2. Housekeeping done at the user's request (earlier in the session)

- Nested Wayland test session (close with
  `pkill -f target-wayland-view/release/flamewm-wayland`).
- Built deb v0.1.7 at `./.target/dist/flamewm_0.1.7_amd64.deb` (not installed).
- README: Displays per-output layout/scale marked Partial.
- Removed 68 stale `/tmp/opencode` worktrees (about 21 GB). Later deleted stale
  scratch dirs older than 4 h when `/` fell to 18 GB; `/` is now about 47 GB
  free, repo filesystem about 428 GB free.

## 3. Landed on origin/main this stretch (verified)

All are ancestors of origin/main with the Sonnet 5.5 trailer unless noted.

| SHA | Slice | Evidence level |
|---|---|---|
| 5eed77c1 | dmabuf default feedback refresh after DRM rebuild | wm-wayland gate 0 pre-rebase; DRM path not run |
| e2a43969 | docs(backlog) reconcile 0a95a8f4..5eed77c1 | guard 0 |
| a546c29e | decision records `docs/backlog/decisions-2026-11-12.md` | docs only; Haiku trailer (not rewritten) |
| d3355c99 | launcher app catalog refresh on open/launch miss | gate 0; unit only |
| 493d4458 | VPN activate/deactivate e2e on mock NetworkManager | gate 0 pre-rebase; private dbus-daemon |
| e3870895 | Enter/Space on open taskbar overflow menu activates first row | shell gate 0; unit only; arrow/focus nav not wired |
| 6dfc17b3 | KDE-style aria-labels on six accent swatches | settings gate 0; static only |
| 02125b39 | polkit dialog submit test | mock-bus e2e passed; final rebase not re-gated; flag still false |
| 1c169efe | BlueZ Agent1 slice 1 (reject-all) on private bus | gate 0 pre-rebase; not wired in shell; no real BlueZ |
| 14094520 | flamewm-lock covers outputs hotplugged while locked | gate 0; unit only |
| 20614c7c | seed lock cover from every layout output, drop removed outputs | gate 0; unit only |
| f85c8eda | docs(backlog) reconcile e2a43969..20614c7c | guard 0 |
| 2457c94a | test(render-x11): stop private-Xvfb tests passing vacuously | gate all 0; 30/30 real runs each |

Earlier landings through 501876cc (5091cdaa, e9751458, 81b133e7, 3d619b81,
53b4d927, d42da107, 0a95a8f4, 6b890a34, 49ca7240, a8c1e394, 501876cc) were
verified in the earlier part of the session.

Last 17 h before this report: 43 commits on origin/main (20 feat, 7 fix, 5 test,
6 docs, 1 refactor, 1 build, 3 other), 183 files, about +5,600 / -470 lines.
No performance commits, no measured speed change; net code grew, so no
simplification claim.

## 4. The deflake finding (2457c94a)

- Symptom: gates failed on clean origin/main at `foreign_event_livelock` (about
  2 of 3 runs, always at a 10 s mark) and `xdnd_two_windows_on_private_xvfb`.
- Finding: the host had 40+ stale `/tmp/.X1xx-lock` files from SIGKILLed Xvfb.
  The old helpers skipped every locked display and printed "skipped", so most
  green runs were vacuous. Real runs failed because the first `XOpenDisplay` on
  a fresh Xvfb stalled.
- Fix: shared `crates/flamewm-render-x11/tests/common/mod.rs` starts Xvfb with
  `-displayfd`, retries `XOpenDisplay` for up to 5 s and keeps a readiness
  client open for the test's lifetime. Only a missing Xvfb binary skips.
- Caveat: the root-cause mechanism (server-init race) is a hypothesis supported
  by 0 failures in 40 runs, not proven. A first worker report of BLOCKED was
  rejected and the worker was resumed with a concrete spec per the
  reference-first rule (standard practice: xvfb-run style `-displayfd`).

## 5. In flight (not on origin/main)

1. Polkit card (worker abe74b79a00f5afea): card renders on private Xvfb; sizing
   fix `d6dc4dbb` (420 px width) is committed locally only. Proof build running
   for the typed-password and Escape run (XTest via Python; `xdotool` not
   installed) against a fake helper on a private mock bus. Temporary
   E2E-PROOF-ONLY hooks must be reverted before landing.
   `POLKIT_AGENT_CAN_ANSWER` (crates/flamewm-shell/src/main.rs ~994) stays
   false until both proofs pass.
2. Displays per-output scale (a9ed2e0ef072ce7cf): surface-to-output table in
   wm-wayland state so fractional-scale uses the surface's output; gate
   running; unit-only (nested winit has one output).
3. X11 magnifier shortcuts (ad10969f582b647f0): `zoom.rs` state with clamp
   1.0..8.0 and 0.5 step; state only, no X11 output scaling yet; gate queued.
4. UiDocument attribute setter (a3a5059cd9cce27ab): `attribute(id,name,value)`
   in render-core/flamewm-ui-document; earlier gate failed clippy
   (`EXIT_CLIPPY=101`); not pushed.

## 6. Unverified claims and known gaps

- Not run in a real session: colour filter on DRM; zoom pointer-follow, DRM and
  X11 output scaling; polkit typed input, Escape and keyboard focus; taskbar
  overflow width model and button/arrow keyboard; per-output scale; mic row
  render; app rescan at runtime; dmabuf on DRM; VPN against live
  NetworkManager; Bluetooth agent (unwired, no real BlueZ); lock hotplug and
  multi-output at runtime (`drop_outputs` on resume/device-pause and wl_output
  removal not handled; wl-lock-multi stays Partial); look-radius covers 1 of
  about 30 sites.
- Landed without a final re-gate after rebase: 6b890a34, 501876cc (final),
  493d4458, 1c169efe, 02125b39.
- Commit hygiene: Haiku trailers on e7cfa0ed, 9750110d and a546c29e; the
  message of 0a95a8f4 contains the wrong phrase "scripts/break-free docs
  slice". Main was not rewritten.
- Backlog rows still saying "decision needed": look-radius-large, read-U8,
  cmp-quick, read-U4, cmp-dock-overflow, perf-bgaudit, pol-session,
  wrss-scratch. Per the reference-first rule these are next to resolve.
- Known load-flaky before 2457c94a: `foreign_event_livelock`,
  `xdnd_two_windows_on_private_xvfb`.

## 7. Next candidates

wl-portal PipeWire ScreenCast follow-ups; dock-slot aria-labels and startup
toggle `aria-checked` once the attribute setter lands; wl-lock-multi
`drop_outputs`; zoom pointer-follow and DRM; taskbar overflow button focus
(needs a ui-x11 keyboard-focus model for shell surfaces); X11 XComposite alt-tab
occlusion; Bluetooth agent shell wiring and UI; cmp-sigblock owner; the
"decision needed" rows above; README Planned/Partial rows; another docs(backlog)
reconcile after the in-flight slices land.
