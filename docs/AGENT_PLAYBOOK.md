# Agent playbook: state, rules and backing information

Status: 2026-11-08, written against origin/main `cd19f519`; section 7 holds this session's lessons. Audience: every CODER worker on FlameWM. Read this before starting or resuming work. It is short on purpose; links go to the owning documents.

## 1. State of the work

### Landed on origin/main
| Area | Commits | Notes |
|---|---|---|
| Dock remake (compact pill, auto-hide, clipped corners) | `cb5999d1` | Later polish (fullscreen reveal, Super+Alt+D, settings toggle, 4-edge shots, PSS) is **not** landed, see 2. |
| Competitor docs: Vela, Xfce, MATE, KDE Plasma | `3f927e37`..`2ac04cac` | `docs/competitors/**`, README rewrite `9dcd373c`. |
| Architecture map, quality scorecard, GPU design | `478cc386`, `b5bc518c` | `docs/ARCHITECTURE_MAP.md`, `QUALITY_SCORECARD.md`, `GPU_BACKEND_SPEC.md`. |
| GPU phase 1 and P1b | `759f8b77`, `c7102230` | `renderBackend` key; legacy `renderAcceleration=gpu` migrates at load. |
| Accent accessor, notification history bounds | `91dfa4db`, `e98e63e5` | One accent source; history keeps no images, text capped 256 KiB. |
| Clipboard history (bounded) | `27076067` | 20 items, 256 KiB. |
| Titlebar double-click, drag modifier | `332c98af`, `e3cccdb3` | Env vars only; settings UI not wired. |
| `notifications.rs` split | `1cbec643` | 1568 to 680 lines. |
| CI action pins, `scripts/lint`, `scripts/gate` | `e6740ec2`, `ab576fa6`, `83ea76ad` | |
| Acceptance fixes SS06/07/12/14/19 | see git log | SS15 reverted, root cause unknown. |

### CI is red, and why
CI failed from `e98e63e5` onward: `cargo clippy --workspace --all-targets --all-features -- -D warnings` reported (a) `len() > 0` in a `notification_history.rs` test, (b) two `assigning_clones` hits in `render_backend_migrate.rs`. Both slipped through because those commits were pushed without a clippy exit 0 for the touched crate. Fixed in this commit (`clone_into`, `!is_empty()`).

### In flight (workers)
| Worker | Task | Waiting on |
|---|---|---|
| dock | polish gates, Xvfb checks, PSS, commit | isolated test build in `target-wt-dock` |
| weather | taskbar tint, centring, dark temperature pill | settings + shell tests |
| GPU | P2a trait + status API, P2b generation counter | combined gate |
| splitter | 7 remaining file splits | per-file gates |
| dedupe/panic hygiene | helper dedupe | clippy log |
| sticky keys / shake | KDE winners 2 and 3 | gates |

### Not started
- Settings DND toggle (plumbing unscoped), KDE winner 4 (workspace OSD, owner unmapped), quick-settings sheet.
- GPU P3a Settings control, P3b `auto`, P4 Vulkan crate, P5 Wayland pixman prototype (`docs/GPU_BACKEND_SPEC.md` section 9).
- Acceptance: SS15 root cause, SS16 re-pin, SS18 and SS08 (env-blocked), final full matrix.
- Phonon toggle, Task Manager idle CPU (2.4% vs <1%), G9/G10/G11, busy-cursor, volume slider drag, `_NET_WM_SYNC_REQUEST`, Vela #163 repro.
- Settings UI wiring for the two Xfce window options.

## 2. Rules every worker follows

1. **Clean worktree only.** `git worktree add /tmp/claude-1000/wt-<name> origin/main`. The shared tree is dirty with others' edits. Never edit, reset or stash it.
2. **One gate, one queue slot.** After `git pull --rebase`, run in the background:
   `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin GATE_LOG=/tmp/claude-1000/<name>-gate.log scripts/gate <crate> [crate...]`
   Wait once: `timeout 580 bash -c 'until grep -q GATE_DONE <log>; do sleep 15; done'`. Push only on `GATE_DONE all=0`. The gate runs guard, fmt, test and clippy `-D warnings`, which is exactly what CI checks, so a green gate means no CI surprise for those crates.
3. **No push without clippy exit 0 on every touched crate.** This is the rule that was broken. Lint-level lints (`len_zero`, `assigning_clones`) only show under `-D warnings`.
4. **Do not duplicate queued jobs.** `pgrep -fa flamewm-terra-cargo`; kill only your own PIDs. Do not start a second target directory unless a measured collision exists: a cold build is about 16 min (`docs/BUILD_TIMES.md`). Cargo hashes include the workspace path, so two checkouts do not overwrite each other in `target-shared`; a test failure is a code failure until proven otherwise.
5. **Wait once, work meanwhile.** No chains of re-armed monitors. While queued: write tests, docs, diffs, measurement scripts, Xvfb scripts.
6. **Commit trailer exactly** `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never `--no-verify`. Push: `env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY git push origin HEAD:main`; on reject `git pull --rebase` and re-gate if sources changed.
7. **Small commits, one deliverable each.** Land a finished package before starting the next.
8. **Report** sha, each `EXIT_*`, and what was not verified. State unrun checks as unrun.

## 3. Quality bar and design contracts

Design rules from the user (apply to every change):
- Animations are opt-in, default off.
- No new glass, blur or shadow effects.
- CPU and memory reductions are always welcome.
- Licences: code stays LGPL-2.0-only. GPL-3 code is reimplemented in your own words. MIT or Apache-2.0 code may be ported with a `THIRD_PARTY_NOTICES.md` entry. GPL-3 theme data ships as a separate aggregate with its own licence text, never linked into code.

- **Memory and CPU:** default `cpu` path allocates nothing new and loads no GPU library; idle wakeups unchanged; session baseline 23.8 MiB PSS (nested Xephyr, 2026-10-30). New features must show PSS before and after (`/proc/<pid>/smaps_rollup`, field `Pss`) or state that they are unmeasured.
- **Look:** blur, surface and outline effects are frozen. Do not change their parameters or layering. Visual changes need screenshots viewed, not assumed.
- **Settings conventions:** camelCase keys; a default value is never serialized; each new key has a round-trip test and an invalid-value test; new settings live in a new small module with a one-line hook (`flamewm-platform/src/settings.rs` is 54 KB and being split).
- **Architecture:** one owner per mechanism, typed facades, `flamewm-reactor` owns native event sources, UI never becomes WM authority. `python3 -I tools/architecture_guard.py` must exit 0 with no new violations. Exceptions are explicit path entries in `docs/ARCHITECTURE_EXCEPTIONS.toml`.
- **Licences:** `.vendor/**` is read-only reference. Vela is GPL-3.0; xfwm4, xfdesktop and KDE Plasma are GPL. Port ideas only, never code.
- **Isolation for runtime tests:** own Xvfb or Xephyr, `pgrep -a Xvfb Xephyr` first, free display in 101..119, explicit `DISPLAY`, never `:0` or `wayland-0`, kill only your own PID. No computer-use or remote control.
- **Dock design contract** (`docs/DOCK_DESIGN.md`): always rounded floating pill, movable to four edges, one display, auto-hide with a 3 px reveal strip, hides for fullscreen, no idle timer when hidden (unmapped, deadline None).
- **Weather design contract:** icon and temperature vertically centred; temperature on a small dark rounded pill so it reads over a bright sun; tint option `weatherTint`, default on, sun yellow, rain blue and so on; stateless colour mapping in `flamewm-weather/src/weather_tint.rs`.

## 4. Per-worker backing information

- **Dock:** if a `render_backend` test fails in your tree, rebase onto origin/main first; phase 1 and P1b changed that area. Verify in Xvfb: fullscreen app hides the dock and reveal strip brings it back; Super+Alt+D toggles; settings toggle "Automatically hide and show the Dock" round-trips. Screenshots for all four edges with several apps. PSS shown vs hidden. A hidden dock must hold zero timers.
- **Weather:** conditions to cover for tint: clear, partly cloudy, cloudy, rain, thunderstorm, snow, fog, night variants. Screenshots tint on and off per condition; PSS delta must be about zero (colour map is a pure function). The default-on tint must not change the dark pill.
- **GPU:** follow `docs/GPU_BACKEND_SPEC.md` literally. P2a/P2b: `GpuPixels` trait, `RenderBackendStatus`, generation counter that drops stale per-thread slots (fixes GPU memory not freed on switch), init-counter unit test proving `cpu` never touches the slot. Do not enable `auto`, do not touch Vulkan, default stays `cpu`.
- **Splitter:** pure moves, public paths unchanged via `pub use`, baseline and after test counts per crate; remaining: `settings.rs`, `layout.rs`, `blur.rs`, `conversion.rs`, `xft.rs`, `surface_controller.rs`, `app/events.rs`. Run workspace clippy once at the end through `scripts/lint`.
- **Dedupe/panic hygiene:** extract shared helpers only where two or more call sites are identical; each change keeps behaviour; prefer `?` over `unwrap` on user-reachable paths.
- **Sticky keys / shake:** pure state machines in `flamewm-platform` first (unit tests per `docs/competitors/kde-plasma/WINNERS.md`), then minimal call-site hooks; sticky keys default off.

## 5. Next work packages (ranked, owners proposed)

1. Land dock, weather, GPU P2a/P2b, splits, sticky/shake (in flight).
2. GPU P3a Settings control and status line (after P2b), then equivalence test calibration (P2c).
3. Settings UI wiring for the two Xfce window options and sticky keys, in the split settings module.
4. Acceptance: SS15 root cause, SS16 re-pin, final full matrix, update `docs/TERRA_RESULTS.md`.
5. Idle CPU: Task Manager dirty-region redraw (<1% target), then G9/G10/G11 features.

## 6. Improving how agents work

| Observed | Cost | Rule now |
|---|---|---|
| Separate queued test, fmt and clippy per worker | 3 to 4 queue slots each; queue peaked at 34 | `scripts/gate`: one slot |
| Chains of re-armed monitors | tokens and noise, no progress | one bounded wait |
| Pushing without clippy | CI red for 6 commits | gate required before push |
| Cold target directories | 16 min builds, disk | shared `target-shared`, `target-clippy189` |
| Idle while queued | wasted wall time | prepare next step in the queue gap |
| Workspace `--all-features` clippy queued early | blocks small jobs | once, at the end, via `scripts/lint` |

## 7. Lessons from this session (2026-11-08)

1. **Toolchain.** Gates need `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin`. The default `cargo` has no clippy, and 1.88 gives false greens. Check `cargo --version` reads 1.89.0 before trusting a result.
2. **Shared-target rlib hazard.** Stale rlibs in the shared target can mislead a build. Use private `GATE_TARGET` and `GATE_CLIPPY_TARGET` for your own gate. Never run `cargo clean` on the shared target.
3. **Disjoint-rebase push rule.** If the gate ended `GATE_DONE all=0` and the incoming origin commits touch none of your files, push without re-gating. The pre-push hook still runs. If any incoming commit touches your files, re-gate.
4. **Verify package names before use.** The window manager package is `flamewm-wm`, not `flamewm`. Check with `cargo metadata --no-deps` or `grep '^name' crates/*/Cargo.toml`.
5. **Kill only your own PIDs.** Never kill another worker's waiter, gate or cargo process. Find yours by the log file or the PID you started.
6. **One bounded wait per gate.** Use one `timeout 580 bash -c 'until grep -q GATE_DONE <log>; do sleep 15; done'`. No re-armed monitors and no polling loops.
7. **Licence rule.** See section 3, Design rules.
8. **Design rules.** See section 3, Design rules. Commit trailer exactly `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
9. **Isolated Xvfb.** Run `pgrep -a Xvfb` first, pick a free display in 101..119, set `DISPLAY` explicitly on every command, and kill only the PID you started.

## 8. Headless runtime harness (private Xvfb, nested flamewm)

Reuse this instead of rediscovering it. Found in `scripts/perf/settings_session.sh` and
`scripts/perf/startup.sh`; no separate canary is needed for a visual check.

- Build: `cargo build -p flamewm-wm-wayland -p flamewm-desktop -p flamewm-shell -p flamewm-settings --bins`
  (debug binaries land in `$CARGO_TARGET_DIR/debug/`).
- Display: pick a `:N` with no live server (`ps -C Xvfb -o args= | grep " :N "`). Never `:0`, and do
  not signal PIDs you did not start. `/tmp/.X11-unix/XN` sockets are often stale and are not a signal.
- Settings: `$XDG_CONFIG_HOME/flamewm/settings.toml` (or `$HOME/.config/flamewm/`), flat lines
  such as `dock.autoHide=false`. Parser: `crates/flamewm-platform/src/settings/snapshot_parse.rs`.
- Start: Xvfb `:N -screen 0 1280x800x24 -nolisten tcp -noreset -ac`, then `env -u WAYLAND_DISPLAY`
  with a private `HOME`, `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, `FLAMEWM_WAYLAND_BACKEND=winit`,
  `FLAMEWM_WAYLAND_AUTOSTART=<desktop>:<shell>:<settings>`, and `setsid dbus-run-session -- flamewm-wayland`.
  The Wayland socket is `$XDG_RUNTIME_DIR/wayland-flame`.
- Extra clients: run the binary with `WAYLAND_DISPLAY=wayland-flame` and the same private env.
- Screenshot: `DISPLAY=:N xwd -root -silent | convert xwd:- out.png`.
- Input: python-Xlib with XTEST (`Xlib.ext.xtest.fake_input`), warping the pointer first.
  Root coordinates are the Xvfb screen, not the nested output.
- Known so far (2026-11-09): the nested `flamewm-wayland` default is 1350x641, which clipped the right side
  and drew a black band against Xvfb 1280x800. Set `FLAMEWM_WAYLAND_WIDTH=1280 FLAMEWM_WAYLAND_HEIGHT=800`
  to match. At 1280x800 with `dock.enabled=true`, the dock draws with autoHide on and off, and the maximized
  reserve differs by 140 px (on 721 px vs off 581 px client height). The earlier stale-pixel symptom was seen
  only at 1350 wide; re-check at 1280 before treating it as a repaint bug.
- Cleanup: kill only the PIDs you recorded (`wm.pid`, `xvfb.pid`) and their process group.
