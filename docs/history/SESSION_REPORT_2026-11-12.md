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

## 8. Addendum: reconcile 7a0d1a4e..6c45665d

Landed on origin/main after the 7a0d1a4e reconcile (18 commits, oldest first).
Claims follow the commit bodies; nothing here was re-run in the reconcile.

- `c9161614` lock covers dropped on DRM pause and resume (unit only).
- `ab25b5ea` ScreenCast refusal e2e on a private dbus-daemon (no frames).
- `8fd8b450` X11 Alt+Tab composite previews for occluded windows (executed Xvfb occlusion proof PASS; no damage invalidation; no minimised or other-workspace previews).
- `984cc167` per-surface output map drives fractional scale per output (unit only).
- `7dbd160b` BlueZ pairing agent slice 2 (queue, 30 s reject; private-bus e2e).
- `e6b84921` BlueZ pairing card and agent registration (executed on private Xvfb with mock org.bluez; real BlueZ unverified; card-side 30 s expiry pending).
- `7dfaf8c0` Wayland zoomFollowMode (unit only at that commit).
- `8df097ac` Wayland magnifier follow runtime proof, `zoomfollow.sh` PASS 18/18 (nested winit; DRM not applied).
- `07c311d4` launcher Tab, Home/End, PageUp/PageDown (unit only; gate exit 0).
- `3be22d73` render-core golden refresh for the polkit card.
- `e53bc0f3` UiDocument typed attribute setter (unit only).
- `f12ad17c` aria-checked on settings toggles, dock aria-label (gate exit 0).
- `9b5c18fa` X11 zoom shortcuts and zoomLevel state (unit only).
- `f44e83b6` dock slot aria-label label fn (unit only).
- `4be2a408` X11 magnifier via RandR CRTC transform (unit only; Xvfb rejects RRSetCrtcTransform).
- `79c2505b` aria roles for polkit card, audio sliders and toast (gate exit 0).
- `c894b3ba` overflow menu arrow, Home and End highlight (unit only).
- `6c45665d` X11 magnifier follow modes, reset on every loop exit, RandR direction (unit only).

Unverified claims, stated as such in BACKLOG.md:

- Real BlueZ and a real pairing: not run (mock org.bluez only).
- Real `polkit-agent-helper-1`, polkitd and pkexec: not exercised (fake helper only).
- X11 magnifier at runtime: unproven; Xvfb rejects RRSetCrtcTransform.
- DRM magnifier, DRM colour filter, dmabuf on DRM: not applied or not run.
- Wayland ScreenCast frames: none; the portal answers refusal only.
- Multi-output and cross-output surface scale: unit only; layer surfaces on secondary outputs and window moves across outputs not covered.
- Alt+Tab: no damage invalidation, no minimised or other-workspace previews.
- Overflow menu highlight and launcher listbox: no visual highlight projection and no aria-selected.
- `install.sh` live install: not run; dry-run only, not in CI.
- `b8e76656` #24 hardening only: double launch not reproduced.

Process notes:

- Haiku trailers (Claude Haiku 5.5 in place of Sonnet 5.5) on `e7cfa0ed`, `9750110d` and `a546c29e`. History not rewritten.
- `b8e76656` carries an extra Haiku trailer line in addition to the Sonnet line.
- The `0a95a8f4` commit message contains the wrong phrase "scripts/break-free docs slice".
- `07c311d4` and a few other commits were pushed before a post-rebase re-gate. The post-push gate for `07c311d4` passed.

## 9. Addendum: reconcile 6c45665d..a5c2ef8d

Landed on origin/main after the 6c45665d reconcile (`14e5d3c9` is the docs-only reconcile; 18 commits, oldest first). Claims follow the commit bodies and the executed runs they name; nothing here was re-run in this reconcile.

- `13e9813e` Bluetooth card shows the Device1 name and the 30 s expiry (executed on private Xvfb; real BlueZ unverified).
- `c6adefe7` layer surfaces and window moves drive per-output scale (unit only).
- `25ddb667` launcher listbox with aria-selected (unit only).
- `dcea2229` wl-portal-frames PipeWire feasibility; blocked on an architecture exception, owner request in the doc (no code).
- `0a435a15` X11 Alt+Tab refresh on repaint and minimised last-frame cache (executed ignored Xvfb tests for damage; minimised cache not executed).
- `ceab2b25` task overflow menu follows the WAI-ARIA menu-button keyboard pattern (unit; keys proven on private Xvfb in `5bec483d`).
- `c666b8ad` layer surfaces map on their output; enter and leave per output (unit only).
- `89959c83` unit coverage for layer output choice, removal and leave (unit only).
- `f952e95d` wlr-layer-shell test client for the nested placement proof.
- `32cd579d` `layer_place.sh` single-output layer placement proof (PASS 25600/25600 band).
- `fbfc4645` Settings output scale applied live on the nested output (`scale_live.sh` PASS, ratio 1.4988).
- `860deeed` saved output scale applied on loop turns, proven live (`scale_live.sh`).
- `d9db1ecd` Caps Lock hint on the Wayland lock presenter (unit only).
- `5bec483d` Start Return launches once; keyboard runtime proof (`shell_keys_proof.py` 23 PASS on private Xvfb). This is the real cause of issue #24; the earlier "not reproduced" claim is corrected and an issue comment was posted.
- `51d474d7` Escape clears the Start query before closing Start (same proof run).
- `20b96d63` quick-controls audio slider keyboard model (unit only).
- `2f28b3bd` notification daemon spec e2e on a private dbus (no gap found).
- `a5c2ef8d` X11 invert colour filter via the RandR gamma ramp (executed gamma round trip on private Xvfb; WM session not run; grayscale and colour-blind are Not applicable on X11 gamma).

Unverified claims, stated as such in BACKLOG.md:

- Real BlueZ pairing (`13e9813e`, `e6b84921`): not verified (`bt-proof`).
- Multi-output placement, scale, lock covers and popups: unit only; the nested backend has one output (`multi-output-runtime`, `popups-on-removed-output`, `work-area-per-output`).
- Launcher and overflow aria values (`25ddb667`, `ceab2b25`, `c894b3ba`): no accessibility-tree run (`launcher-aria`, `overflow-aria`).
- Quick-controls slider keys (`20b96d63`): unit only (`slider-keys-proof`).
- Minimised Alt+Tab cache (`0a435a15`): no executed run recorded (`alttab-min`).
- X11 magnifier (`6c45665d`, `4be2a408`, `9b5c18fa`): unit only; Xvfb rejects RRSetCrtcTransform (`x11-zoom-proof`).
- DRM magnifier and colour filter: not applied (`drm-zoom-filter`).

Process notes:

- `860deeed` carries a `Claude Haiku 5.5` trailer instead of `Claude Sonnet 5.5`. Not rewritten; main is shared.
- A worker report that `zoomfollow.sh` was absent was wrong. The script exists (`scripts/break/zoomfollow.sh`) and ran PASS 18/18 in `8df097ac`.
- Commits pushed before a final rebase re-gate: `4be2a408`, `f44e83b6`, `79c2505b`, `25ddb667`, `9b5c18fa`, `ceab2b25`, `89959c83`, `860deeed`, `5bec483d`. Their gate closure is unchanged by inspection; they were not re-gated after the final rebase. `07c311d4` was re-gated after its push.
- The earlier launch-once "not reproduced" claim was wrong; corrected in `5bec483d` and in BACKLOG.md.

## 10. Addendum: reconcile a0ebcdad..e265bda0

Landed on origin/main after the a0ebcdad reconcile (14 commits, oldest first). Claims follow the commit bodies and the runs they name; nothing here was re-run.

- `5d46bf0e` X11 tiling, snap preview and titlebar double-click proof (executed 15/15 on build a0ebcdad).
- `dc3f121f` X11 clipboard manager: not provided (no code; grep finds no selection hook in `flamewm-wm-x11`).
- `01bb2495` Wayland clipboard and PRIMARY text kept after the owner exits (7 unit tests and `clipboard_persist.sh` PASS, executed; text only).
- `6dca4ffc` audio slider keys step from the last target; focus ring; `shell_keys_proof.py` 32 PASS (executed, private Xvfb).
- `743adbb2` nested-Wayland tiling and snap proof script (run result not in the commit body).
- `c3e9d2c4` Alt+F7/F8 geometry; `_NET_WORKAREA` excludes the Dock reserve (unit).
- `d9305d17` X11 Alt+F7/F8 keyboard move and resize; section 2b 23/23 in four runs, 22/23 in one (pointer drag-away check).
- `e265bda0` Wayland Alt+F7/F8 keyboard move and resize (unit only; no nested proof, no pointer warp).
- `54d21102`, `91400f0a`, `f5261e1e`, `951e66ed` AI usage data path (unit only; UI and tray card pending).
- `12e16203` settings keyboard proof on a private bus; known failing Tab ring check (stop 17).
- `7be74efe` Ctrl+Q closes Settings; Escape clears the sidebar search; sidebar focus ring (proof not clean at that commit).

Unverified claims, stated as such in BACKLOG.md:

- Wayland tiling 12/12 (`743adbb2`): the figure is not in any commit body (`wl-tiling-proof`).
- Settings Tab focus ring (`12e16203`, `7be74efe`): failing; no fix commit in range; the "fixed in the next slice" note has no matching commit (`settings-keys-proof`).
- X11 Alt+F7/F8 (`d9305d17`): one run 22/23 on the drag-away check; no FocusOut ungrab in the body (`kbd-move-x11`).
- Wayland Alt+F7/F8 (`e265bda0`): no runtime proof, no pointer warp (`kbd-move-wl`).
- Usage card (`951e66ed` and the three before it): no UI, no live provider call (`usage-card`).

Process notes:

- A worker once started a settings binary outside a private bus; the probe reached the host bus. No known write. Not a product bug (`12e16203` body).
- Commits with a `Claude Haiku 5.5` trailer instead of `Claude Sonnet 5.5`: `e7cfa0ed`, `9750110d`, `a546c29e`, `860deeed`. Not rewritten; main is shared.
- `8fde93ca` was claimed pushed. It is not an ancestor of origin/main; only its rebased copy `c3e9d2c4` reached main, with the same subject.
- This report lives at `docs/history/SESSION_REPORT_2026-11-12.md` (the brief named `docs/`; the path was not moved).
