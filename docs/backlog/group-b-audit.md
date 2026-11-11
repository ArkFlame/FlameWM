# Group B backlog audit

> Rows: `wsh-grab`, `wsh-strut`, `meas-blur`, `meas-settings`, `meas-terminal`, `meas-build`,
> `meas-perf-blocked`, `meas-heap`, `meas-sign`, `meas-tm`, `meas-shadow`, `meas-blur-eq`,
> `wrss-scratch`, `wrss-gpu`, `pol-helper`, `bt-agent` in `docs/BACKLOG.md`.

Base: `origin/main` at `6e589404` (worktree `/tmp/opencode/wt-groupb`, detached). Method: premise checked against
current source, landed SHAs checked with `git merge-base --is-ancestor`, and runtime checks run only in private
sessions. Scope limits: no edits under `crates/flamewm-wm-wayland/**` or `crates/flamewm-settings/**` (owned by
another worker), no `crates/flamewm-render-x11/**` edits, `docs/BACKLOG.md` not edited.

This file lands no product code. Every row below ends either landed (ancestor verified) or documented blocked with
the decision needed.

## Environment facts (checked this pass)

| Fact | Evidence |
| --- | --- |
| `perf_event_paranoid` is 4 | `cat /proc/sys/kernel/perf_event_paranoid` -> `4` |
| `perf record` cannot open events | `perf record -o <tmp> true` -> "Failure to open any events for recording" (exit non-zero) |
| `python-Xlib` is absent | `python3 -I -c 'import Xlib'` -> `ModuleNotFoundError`, exit 1. `scripts/perf/startup.sh:11` requires it |
| Xvfb, dbus-daemon, xdpyinfo, cargo present | `/usr/bin/Xvfb`, `/usr/bin/dbus-daemon`, `/usr/bin/xdpyinfo`, `/usr/bin/cargo` |
| DRM device nodes exist | `/dev/dri`: `card0`, `card1`, `renderD128`, `renderD129` |
| weston absent | `which weston` -> not found |

## Per-row table

| row | premise verdict | landed SHA / blocker + decision | EXIT lines | unit vs runtime |
| --- | --- | --- | --- | --- |
| wsh-grab | Confirmed. X11 is a frame-event reducer (`crates/flamewm-wm-x11/src/frame/controller*`, `frame/geometry/moves.rs`, `MOVE_ACTIVATION_THRESHOLD_PX`). Wayland is a pointer-state enum `enum Grab` at `crates/flamewm-wm-wayland/src/state.rs:60` | Blocked. L size, needs a dedicated slice plan. Decision: owner picks the shared model (a pure move/resize transition function in `flamewm-window-core` taking pointer deltas, with both backends as adapters). Wayland side is owned by the other worker, so the adapter step cannot land from this group | none run (no code change) | n/a |
| wsh-strut | Confirmed as "different inputs". X11 folds `dock_struts` with range checks (`crates/flamewm-wm-x11/src/wm/outputs.rs:112-150`, `work_area` near `:124`). Wayland uses layer-shell exclusive zones (`crates/flamewm-wm-wayland/src/state.rs:732` `work_area`, `:1195`, `:1249` `work_area_with_dock`). Line numbers in `docs/backlog/wayland-shared.md` row 4 are stale | Blocked. M size. Decision: owner confirms the two inputs stay separate. A shared fold would take a normalized `[i32; 4]` strut list in core; the Wayland call site is in the other worker's crate | none run | n/a |
| meas-blur | Confirmed. `docs/backlog/measure.md:44` ranked item 2; blur is 77-83 ms of a 83-93 ms batch (estimate, CPU path). GPU path unmeasured (`flamewm-gpu-helper` not found) | Blocked. L size, needs a dedicated slice plan. Decision: owner picks CPU blur removal vs GPU path. No runtime number taken this pass | none run | n/a |
| meas-settings | Confirmed (figures from an earlier pass, not re-measured): `measure.md:45` 1774/1844 ms map/paint; cold after shadow cache 463 ms painted median (`measure.md:62-70`). Not profiled | Blocked (runtime). `scripts/perf/startup.sh` needs `python-Xlib`, absent. `perf record` blocked by `perf_event_paranoid=4`. Decision: owner installs `python-Xlib` in a private venv (or approves it) and lowers `perf_event_paranoid` or grants `CAP_PERFMON` for one run | none run | runtime blocked |
| meas-terminal | Premise superseded. The 276/618 ms figures are historical; the later re-measure is `d36e27db` (ancestor: yes), 24.3 ms median map-to-paint over seven runs. `docs/backlog/meas-terminal-audit.md` marks the ~340 ms gap as stale | Documented, close candidate. Decision: owner closes the row as not-reproduced on `d36e27db`, or orders a same-harness re-run. No fix is justified by current numbers | none run | runtime blocked (same harness gap) |
| meas-build | Confirmed as measured. `measure.md:143` meas-build: gate scope lacks `syn`/`x11rb`; build-override lever -6.5%, below the 10% bar | Measured, no lever. No action. Decision: owner confirms close | none run | n/a (compile numbers from earlier pass) |
| meas-perf-blocked | Confirmed. `perf stat` runs, `perf record` fails: "Failure to open any events" with `perf_event_paranoid=4` | Blocked (environment). Decision: owner lowers `perf_event_paranoid` to 2 or grants `CAP_PERFMON` on this host. Not changed here: a host sysctl is outside this group | `perf record -o <tmp> true` non-zero | runtime blocked |
| meas-heap | Confirmed. `e92adbc5` ancestor: yes. Attribution only (about 10.4 MB non-Rust malloc; no site over 5 MB) | Landed `e92adbc5`. No fix | n/a (docs) | n/a |
| meas-sign | Confirmed. `f7f8a56f` ancestor: yes. Test pins root-relative origin against xwininfo | Landed `f7f8a56f`. Re-run this pass: `python3 -I tools/test_flamewm_frame_origin.py` | EXIT 0 (`OK`) | unit (python, pass) |
| meas-tm | Confirmed. `c759a2a8` ancestor: yes. "startup probe screen fits task-manager so it paints" | Landed `c759a2a8`. Not re-measured: startup harness blocked (python-Xlib) | none run | runtime not re-measured |
| meas-shadow | Confirmed. `819a2284` ancestor: yes ("cache finished text-shadow images") | Landed `819a2284`. Settings cold paint 463 ms baseline not re-measured (harness blocked) | none run | runtime not re-measured |
| meas-blur-eq | Confirmed. `852f81ce` ancestor: yes ("recalibrate T_BLUR to 21") | Landed `852f81ce`. Tests not re-run this pass | none run | not re-run |
| wrss-scratch | Premise conflict in the docs (see below). Scratch texture drop measured at -3.0 to -3.5 MB PSS, below the 5 MB bar | Blocked (decision). Decision needed: owner records one status: open (fixable, needs runtime count of the wallpaper raster) or closed as not-fixable (below bar). Inconsistency is recorded below | none run | runtime not re-measured |
| wrss-gpu | Confirmed. All wayland-rss numbers are llvmpipe (`wayland-rss.md:13`, `:106`). `/dev/dri` nodes exist on this host | Blocked. Decision: owner approves a private GPU-host run. The measurement method is not a committed script, and the startup harness needs python-Xlib | none run | runtime blocked |
| pol-helper | Confirmed. No helper I/O exists. `crates/flamewm-integrations-linux/src/polkit_agent.rs:6` says the password step is out of scope. Threat model below | Blocked, security-sensitive. Not implemented. See threat model | none run | n/a |
| bt-agent | Confirmed. No `org.bluez.Agent1` code exists (`grep -rn Agent1 crates` empty). `BluetoothDeviceControl` at `crates/flamewm-integrations-linux/src/bluetooth.rs:375` connects only the system bus client (`:387`). Threat model below | Blocked, security-sensitive. Not implemented. See threat model | none run | n/a |

## Doc inconsistency: wrss-scratch (decision needed)

Exact text that disagrees:

- `docs/backlog/wayland-rss.md:17`: "ARCHITECTURAL, fixable, below bar | Measured below. Not landed."
- `docs/backlog/wayland-rss.md:43`: "real but below the 5 MB bar ... Revisit if a larger idle target is needed."
- `docs/BACKLOG.md` row `wrss-scratch`: "open ... Status conflict: text says both open and closed as not-fixable; DECISION NEEDED."
- `docs/backlog/wayland-rss.md:62`: the 3.38 MB `image_core` buffer is "likely the same buffer (not confirmed)" as the scratch texture.
- `docs/BACKLOG.md` row `wrss-scratch`: the 3.38 MB buffer "is not the scratch texture".

Decision needed from the owner: (1) the status (open or closed as not-fixable), and (2) which of the two statements about the 3.38 MB buffer is correct. Only a runtime count can settle (2). No code change is made here.

## Threat model: pol-helper (polkit password flow)

Status: not implemented. Size L. Security-sensitive.

What exists in source (current `origin/main`):

- `crates/flamewm-integrations-linux/src/polkit_agent.rs:6`: "password step (polkit-agent-helper-1) is out of scope here".
- `polkit_agent.rs:17-27`: fixed Authority and Agent constants (`org.freedesktop.PolicyKit1`, agent path `/org/flamewm/PolkitAgent`).
- `polkit_agent.rs:49-53`: `AuthPrompt` trait (UI boundary; `show` may refuse).
- `polkit_agent.rs:57`, `:71`: bounded `PolkitEventQueue` (`MAX_QUEUED_EVENTS` = 16), no bus.
- `polkit_agent.rs:136-169`: `AgentSessions` keyed by cookie; duplicate cookie rejected; cancel dismisses once.
- `crates/flamewm-shell/src/polkit_dialog.rs:13`: `MAX_PASSWORD_BYTES` = 1024. `:26-30`: `PasswordField` typed buffer with wipe on clear and `Drop` (`:80`). `:86-88`: Debug prints length only. `:103-113`: submit type that carries the password bytes out of the dialog.
- `packaging/arch/PKGBUILD:62-63` and `packaging/debian/control.in:9`: third-party agents only (`polkit-gnome`, `polkit-kde-agent`) are suggested. FlameWM ships no helper.

What must be designed (no code exists for any of these):

1. Helper invocation: which binary path, how it is located and verified (owner-owned, setuid, root), argv, environment, and whether it is spawned per request or kept alive.
2. Pipe protocol: stdin/stdout framing for the PAM prompts and the response, timeouts, and handling of helper exit codes.
3. Cookie and identity: which `Identity` is passed to the helper, and proof that the cookie belongs to the pending request (`AgentSessions`).
4. Secret lifetime: where the password bytes live between the dialog and the helper write, who zeroes them, and that no log, Debug, or wire encoding ever includes them.
5. Cancel races: a cancel arriving during a helper round trip, and a helper that outlives its request.
6. Rate limiting and lockout: retries are owned by polkit or by FlameWM. The helper's own policy is authoritative.
7. Packaging: whether the setuid helper is shipped, or the feature depends on a third-party agent.

Decisions needed from the owner:

- D1: Ship a FlameWM helper client, or rely on `polkit-gnome` or `polkit-kde-agent` only?
- D2: May the shell hold a password string at all, or must the password go straight from the dialog to the helper pipe without a `String`?
- D3: Helper path and packaging (setuid root binary owned by the distro package, or a FlameWM-owned binary).
- D4: Failure policy when the helper is missing (refuse all auth, which is the current effective behavior, or fall back).
- D5: Owner approval for a live polkit run on a private session bus. No system-bus probing is allowed.

Safe incremental first slice (no password I/O): wire `PolkitEventQueue::drain()` into the shell as a display-only prompt, with the answer always "cancel". This keeps the current behavior (all auth denied) and needs no secret handling. It belongs to row `pol-ui`, not `pol-helper`, and does not satisfy `pol-helper`.

## Threat model: bt-agent (BlueZ pairing agent)

Status: not implemented. Size L. Security-sensitive.

What exists in source:

- No `Agent1` code exists. `grep -rn Agent1 crates` returns nothing.
- `crates/flamewm-integrations-linux/src/bluetooth.rs:95-98`: `device_op_message` builds `org.bluez.Device1.Pair`.
- `bluetooth.rs:375`, `:387`: `BluetoothDeviceControl` with `BusPump::connect(BusKind::System)` (reactor-owned system bus).
- `bluetooth.rs:431` `on_ready` consumes replies; error replies are logged once.
- `docs/backlog/bluetooth.md:23`: `bt-agent` is the first open row; without it most devices fail to pair.

What must be designed:

1. Agent object: exported `org.bluez.Agent1` path, registered with `RegisterAgent(path, capability)` and `RequestDefaultAgent`. Capability choice (`KeyboardDisplay`, `DisplayYesNo`, `NoInputNoOutput`) decides which prompts BlueZ sends.
2. Whether `flamewm-dbus-reactor` can export method handlers. This is not verified; the current code only sends calls and reads replies.
3. Callback set: `RequestPinCode`, `RequestPasskey`, `DisplayPasskey`, `RequestConfirmation`, `Authorize`, `Cancel`, `Release`. Each needs a defined answer or rejection.
4. Binding: accept a prompt only while FlameWM has a user-initiated `Pair` pending for that device path. Otherwise reject.
5. Secret handling: PIN and passkey bytes are returned to BlueZ and must be wiped after use, and never logged or put in a snapshot (`BluetoothSnapshot` is published to the control wire).
6. Caller identity: BlueZ calls the agent, so the reactor must check the sender of each incoming call is `org.bluez`. Registration on the system bus is not authenticated by BlueZ.
7. Timeouts and cancel: what happens if the user never answers, and how `Cancel` from BlueZ is handled.

Decisions needed from the owner:

- D1: Capability string (`DisplayYesNo` and passkey confirmation UI, or `NoInputNoOutput`).
- D2: Accept only during a user-initiated pair (recommended), or also for unsolicited pairing requests.
- D3: Whether the reactor may export object methods (new reactor capability; the reactor owns native event sources).
- D4: Sender policy: accept only `org.bluez` as sender for agent callbacks.
- D5: Owner approval for a live pairing test. No pairing against a real device or the host bus is allowed in this group.

Safe incremental first slice: none fixes pairing. A rejecting agent (registers, rejects every callback, reports the reason through the existing error log) would be safe, but it does not satisfy `bt-agent`'s acceptance and should not be counted as landed.

## Commands run in this pass

| Command | Exit |
| --- | --- |
| `git -C /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender fetch origin` | 0 |
| `git worktree add --detach /tmp/opencode/wt-groupb origin/main` | 0 |
| `git merge-base --is-ancestor <sha> origin/main` for f7f8a56f, c759a2a8, 819a2284, 852f81ce, e92adbc5, edd3d8ca, d36e27db | 0 each |
| `python3 -I tools/test_flamewm_frame_origin.py` | 0 |
| `python3 -I -c 'import Xlib'` | 1 (absent) |
| `perf record -o <tmp> true` | non-zero (paranoid=4) |

No Cargo build or gate was run, because no code changed.
