# Terra handoff 2 (adds to `docs/PAIR_OPENCODE_TERRA.md`; both stay in force)

Date: 2026-11-07. Repo `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender`, branch `main`.

## 0. Operating contract (read twice)

You are a senior Rust/desktop engineer working autonomously. **Your job is to finish, not to report blockers.**

1. **Never stop on a failure.** On any failing command or test: state symptom, form one hypothesis, test it with one
   command, fix the cause, rerun. Three failed fixes on the same symptom: change approach (different hypothesis, smaller
   scope, or work around), do not retry the same thing. Only stop for: a destructive action not yet authorised, or a
   decision that needs the user's taste (UI direction). Everything else you resolve yourself.
2. **No "needs Claude" dead ends.** Section 1 grants you the files you were blocked on. If you still need a file owned by
   Claude, implement the smallest self-contained version inside your own paths (adapter, trait, feature flag) and list the
   follow-up request; keep working.
3. **Evidence over claims.** Every "done" needs a command, its real exit code, and (for UI) a screenshot you looked at.
   Static inspection is not proof. If something is unverified say so in one line, then verify it if at all possible.
4. **Definition of done per goal:** code + unit tests + executed acceptance (script or manual run with output) + zero rustc
   warnings + `python3 tools/architecture_guard.py` exit 0 + committed (own paths only) + pushed + a section in
   `docs/TERRA_RESULTS.md`.
5. **Lazy senior developer:** reuse existing crates/helpers (grep before writing), smallest correct change, no speculative
   abstractions, files under 700 lines, no new always-on threads or sub-50 ms idle timers, bounded caches.
6. **Serialise heavy builds:** `flock /tmp/opencode/flamewm-terra-cargo.lock env CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-terra CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo ...`. The acceptance scripts run cargo internally: export the
   same `CARGO_TARGET_DIR/CARGO_BUILD_JOBS/RUST_TEST_THREADS` before invoking them, or wrap with flock.

## 1. Fixes for what you reported (do these first, now)

**R1. Platform transactions (primary output, orientation, arrangement, 15 s keep/revert).** You are now the owner of:
`crates/flamewm-platform/src/services/display.rs`, `crates/flamewm-platform/src/host/displays.rs` (+ their tests), and the
display facade files in `crates/flamewm-control-core` / `flamewm-control-dbus` / `flamewm-control-wire` that you must touch for
new display requests. Edit them with small exact-string edits (re-read right before editing; another worker edits
`flamewm-platform/src/settings.rs` and `flamewm-shell`, do not touch those). Implement typed requests:
`SetPrimary(output)`, `SetOrientation(output, rotation)`, `SetArrangement(layout)` through the same transaction model as
the mode change: validate topology, apply via RandR (X11), arm the 15 s keep/revert, rollback on failure, tests for validation
and rollback with a fake host. Multi-output on Xvfb is impossible: use a fake `DisplayHost` in unit tests and mark runtime
multi-output as ENVIRONMENT_BLOCKED (acceptable), but single-output orientation/primary must run on Xvfb.

**R2. Wayland scale persistence.** The Wayland WM reads the key `scale.wayland-0` (see
`crates/flamewm-wm-wayland/src/control/settings.rs`, read-only for you). In settings/platform, when the active session is Wayland
write the selected output's scale to `scale.wayland-<index>` AND `scale.wayland-0` for the first output, treat persistence
failure as a failed transaction (rollback). Unit-test it.

**R3. SS16 renderer-promotion failure is not yours.** Verify once on a clean checkout
(`git worktree add /tmp/opencode/clean HEAD` then run `python3 tools/verify_rwr_promotion.py` there; remove the worktree after)
that it fails identically at HEAD. If yes, record "pre-existing at HEAD" with the evidence path and exclude SS16 from your
gate. Do not edit renderer files.

**R4. SS00 "observed concurrent git changes" and SS10 "display outside :97..:119".** SS00: run the acceptance runner on a
clean worktree copy of your commit (or pass the runner's flag to ignore concurrent changes if it has one) so concurrent
workers do not fail it. SS10: start Xvfb on a display inside `:97..:119` (`pgrep -a Xvfb` first, pick a free one) and rerun.
Rerun `scripts/settings-acceptance` until every failure is either fixed or proven pre-existing/environment-blocked with evidence.

**R5. G3 screenshot (in progress).** Finish it per `PAIR_OPENCODE_TERRA.md` G3. `notify.rs` and `scripts/screenshot-acceptance`
are yours. Run the acceptance script, fix failures, commit own paths, push.

Commit and push R1 to R5 as soon as green. Then continue with section 2.

## 2. New goals (bigger features, in priority order, each its own commit series)

Pick by user value; do them in this order unless blocked. Each is one deliverable of real size.

### G6. Settings: Startup Applications, Default Applications, Keyboard Shortcuts
New pages in `ui/settings.html` + `flamewm-settings`/`-core`:
- **Startup Applications:** list/enable/disable/add/remove XDG autostart entries (`~/.config/autostart/*.desktop`, honour
  `Hidden=true` / `X-GNOME-Autostart-enabled`), add from installed apps or custom command, per-entry delay.
- **Default Applications:** browser, mail, file manager, terminal, image viewer, video, audio, text editor via
  `xdg-mime`/`mimeapps.list` read-write with correct MIME groups; detect installed handlers from `.desktop` files.
- **Keyboard Shortcuts:** list the WM's actions and their bindings (read the existing keybinding config via the control
  facade; if no read API exists add a read-only listing request in your granted platform/control files), editable chords with
  conflict detection and reset to default, persisted in the existing settings store. Do not change WM key handling (Claude's).
Unit tests for parsing/serialisation (desktop-entry and mimeapps round trips with fixtures), conflict detection, and an
acceptance extension.

### G7. Finder: universal search (launcher-grade)
Upgrade `flamewm-finder`: instant results from installed applications (desktop entries with fuzzy + frecency ranking), files
(background index with inotify, bounded memory, excludes), calculator (`2+2*3`, units/currency-free conversions like `5km in mi`),
recent documents, system actions (lock, logout, suspend). Keyboard first: type, arrows, Enter, Tab for actions. Index builds off
the UI thread, cached on disk, incremental. Benchmark: first-result latency on a 200k-file tree and index RSS; report numbers.

### G8. Terminal: tabs, splits, search, profiles
`flamewm-terminal`: tabs (Ctrl+Shift+T/W, reorder, rename), horizontal/vertical splits, find-in-scrollback (Ctrl+Shift+F),
profiles (font, colours via existing tokens, shell, working-dir inheritance), URL detection with Ctrl+click, bracketed paste
safety, scrollback memory cap. Measure scrollback throughput (`yes | head -n 1000000` time, RSS) before/after.

### G9. Screen recording (extend flamewm-screenshot)
Region/full-screen/window recording to MP4/WebM/GIF using the X11 SHM capture path from G3 and `ffmpeg` if installed (detect,
degrade with a clear message; no hard dependency). Start/stop with a visible stop indicator (small notification action), audio
off by default with optional PulseAudio monitor source. Bounded CPU: capture on a worker thread at a fixed FPS, drop frames
rather than queue unboundedly. Test with a synthetic frame source; measure CPU % at 30 fps 1080p.

### G10. Image Viewer: viewing and light editing
`flamewm-imageviewer`: filmstrip of folder, slideshow (no animation by default: instant cut), zoom/pan fit modes, rotate/flip
lossless-in-view with save, crop, EXIF panel, copy to clipboard (public `flamewm-clipboard` API), delete to trash with undo.
Large image decode off the UI thread with progressive downscale; report time-to-first-pixel on a 40 MP file.

### G11. Task Manager: process tree, per-process stats, startup impact
`flamewm-task-manager`: process tree view with kill tree / signals menu, per-process CPU/memory/IO/threads columns sorted
cheaply (read `/proc` incrementally, no full rescans every tick; sample at 1 s only while the window is visible, pause when
hidden), search/filter, GPU usage where `/sys`/nvidia-smi is available (optional, detect), and the app's own idle CPU must be
under 1 %. Report idle CPU/RSS before/after.

### G12. Archiver
`flamewm-archiver*`: formats zip, tar.{gz,zst,xz,bz2}, 7z (if `7z`/`7zz` present), password-protected zip, extract here /
extract to folder / compress selection, progress with cancel, zip-slip and symlink-escape protection with tests (security
critical: write explicit traversal tests).

## 3. Guardrails that remain from handoff 1 (summary)

- Own only your paths + section 1 grants. Commit only your paths (`git add <paths>`, never `-A`), `git pull --rebase --autostash
  origin main` before each push, never force-push, never touch `Cargo.lock` unless the diff is only your crates.
- Gate before each push: crate tests exit 0, `cargo check --workspace` zero `^warning`, `python3 tools/architecture_guard.py` exit 0.
- Visual checks only on an isolated Xvfb display you started, kill only your PIDs, never touch the user's real session.
- No visual redesign: use existing tokens; new colours need a token request (Claude owns `ui/shell/flamewm.css`/`shell_tones.rs`;
  `ui/settings.css` is yours, so prefer tokens that already exist).
- Claude is currently building the macOS-style Dock (`flamewm-shell`, `flamewm-shell-core/src/dock.rs`, `ui/shell/dock*.html`,
  platform settings keys `dock.*`/`taskbar.*`). When it finishes, `docs/DOCK_SETTINGS_SPEC.md` lists the Settings controls you
  must add to `ui/settings.html` ("Dock" group, taskbar enable/style/gradient). **Add that as G6b once the spec file says
  "READY"; until then skip it.**

## 4. Reporting format (append per goal to `docs/TERRA_RESULTS.md`)

```
## G<n> <title> — DONE | PARTIAL | BLOCKED(env)
- Changed: <paths>
- Commands: <command> → exit <n>   (real exit codes)
- Evidence: <log/screenshot paths>, numbers before/after
- Not verified: <one line>
- Risk: <one line>
- Commit: <hash> pushed: yes/no
```
Status DONE requires every item in section 0.4. PARTIAL must list the exact remaining step and you must continue it in the same
session, not hand it back.

## 5. Loop protocol

For each goal: (1) read owner files and existing tests (grep first), (2) write the failing test or acceptance, (3) implement,
(4) run focused tests, (5) run the real thing and look at it, (6) fix, (7) gate, (8) commit own paths and push, (9) report, (10)
next goal. Keep one active step at a time. Never leave the tree uncompilable for more than one command cycle.
