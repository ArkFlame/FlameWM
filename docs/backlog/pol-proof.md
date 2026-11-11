# Runtime proof with a real polkit daemon and pkexec (pol-proof)

Status: **UNVERIFIED** - no real daemon round trip was executed. This file records the
observed obstacles, the source-based reachability result, and the smallest next action.
Static source plus executed probes only; nothing here is runtime proof of the agent.

Backlog row: `pol-proof` in `docs/BACKLOG.md`. Related open row: `pol-helper`
(`polkit-agent-helper-1`), which this task deliberately does not implement.

## What the in-tree core can and cannot do

`crates/flamewm-integrations-linux/src/polkit_agent.rs` (668 lines) owns:

- message construction: `register_message` (L187), `unregister_message` (L202),
  `session_subject` (L216), `authority_call` (L211);
- agent-side dispatch: `handle_agent_call` (L270) for BeginAuthentication and
  CancelAuthentication, `AgentSessions` (L137), `PolkitEventQueue` (L60),
  `AuthPrompt` (L47);
- session-id resolution: `resolve_session_id` (L386) with a `SessionLookup` trait.

Grep over the file finds no `AuthenticationAgentResponse2` sender, no `Command::new` or
`process::Command`, and no `polkit-agent-helper-1` invocation. The module doc (L1-L7) says
the password step is out of scope, and the UI side is `show`/`dismiss` only: the core never
produces an authentication answer.

Consequence for the protocol: the agent interface is inbound. The authority sends
`BeginAuthentication(cookie)`; the agent can only convert it into authorization by writing
the response through `polkit-agent-helper-1` (documented at `docs/backlog/polkit.md:18`).
The in-tree agent can at best receive a real request and cancel it through
`Authority.CancelAuthentication`; it cannot complete one.

## Reachability, from the protocol

1. `CheckAuthorization` with user interaction disabled never selects an agent, so it cannot
   produce `BeginAuthentication`. There is no "authorization-only" agent trigger.
   (Protocol knowledge; no polkit source exists in this repo, so this item is INFERRED, not
   verified here.)
2. `pkexec` performs an interactive check, so it does reach a registered agent - but that
   agent must be registered for the caller's session subject, which needs a live logind
   session and a root-owned authority.
3. Even when `BeginAuthentication` arrives, finishing `pkexec` needs the helper. Without
   `pol-helper` the best observable outcome is "prompt received, then cancelled or failed".

## Probes executed (2026-11-11, this host)

Host facts, read-only:

- `polkitd 127-3ubuntu1` at `/usr/lib/polkit-1/polkitd`, running as user `polkitd` under
  systemd; `/usr/lib/polkit-1/polkit-agent-helper-1` present with
  `mode=755 uid=0 gid=0` (no setuid bit).
- A live KDE agent is registered for uid 1000 (`polkit-kde-authentication-agent-1`, PID
  2503). Registering the in-tree agent on the host bus would hijack real user prompts, so it
  was refused.
- `ldd /usr/lib/polkit-1/polkitd` links `libsystemd.so.0`, and `strings` shows only
  `../src/polkitbackend/polkitbackendsessionmonitor-systemd.c` (no `-unix.c` variant).
  This build resolves every subject through logind; there is no utmp or /proc fallback.
- `kernel.apparmor_restrict_unprivileged_userns = 1`; `unshare -Ur` fails with
  `write failed /proc/self/uid_map: Operation not permitted`.
- `sudo -n true` reports `interactive authentication is required`.

Isolated-daemon attempts:

- Private system bus (`dbus-daemon` on `unix:path=/tmp/opencode/polproof/bus.sock`,
  `DBUS_SYSTEM_BUS_ADDRESS` pointed at it, permissive config): polkitd as uid 1000 exits
  immediately with `Error switching to user polkitd: Error clearing groups: Operation not
  permitted`, `rc=1`. polkitd 127 unconditionally drops privileges to its service user,
  which requires real root.
- Rootless user namespace, the only way to fake that root without sudo, is blocked (above).
- So a private `polkitd` cannot be started here, and the host daemon cannot be used without
  touching real user sessions.

Incident, recorded so it is not repeated: one read-only
`Authority.CheckAuthorization(unix-session 6, org.freedesktop.policykit.exec, flags=0)` call
against the host daemon returned `Remote peer disconnected`. The journal shows
`polkit.service: Main process exited, code=killed, status=6/ABRT` at 00:47:19 followed by a
systemd restart (`Started polkitd version 127`). No polkit configuration changed and no
package was installed, but the host service did abort and restart once. Do not probe the host
authority again for this task.

## Why UNVERIFIED rather than a pass

- No private polkitd: needs real root (setgroups) and logind; both unavailable here.
- No session subject without logind: the packaged polkitd only has the systemd session
  monitor, so even a containerised or `dbus-run-session` daemon could not register or select
  an agent for a session subject.
- No completion without `pol-helper`: the agent has no response path, so `pkexec` could
  never return success even if a request arrived.

## What the unit tests do prove, and do not

The `#[cfg(test)]` modules cover `parse_begin`/`parse_cancel` decoding, duplicate-cookie
rejection, queue bounds and dismiss ordering, `handle_agent_call` dispatch of synthetic
`Message` values, and `resolve_session_id` against a fake `SessionLookup`. They build
`dbus::Message` structs in process: no bus, no authority, no agent registration. The
"mock-bus tests only" wording in `docs/BACKLOG.md` is generous - there is no bus fixture at
all.

## What would count as proof

In a disposable systemd environment (VM or a privileged systemd container, isolated from this
host's bus), with polkitd, logind and a non-root test user:

1. The agent registers (a real `RegisterAuthenticationAgent` accepted by the daemon) and
   `resolve_session_id` returns the real logind session of the agent pid.
2. `pkexec /bin/true` from that non-root session makes the daemon send
   `BeginAuthentication` for `org.freedesktop.policykit.exec`, and the queue emits
   `PolkitEvent::Prompt` with the cookie.
3. Cancel path: `Authority.CancelAuthentication` makes the `pkexec` call fail with
   `org.freedesktop.PolicyKit1.Error.Cancelled` or `NotAuthorized`, closing the round trip.
4. Success path: only after `pol-helper` lands - helper spawn, PAM exchange, `pkexec`
   returning 0.

## Smallest next action

Add an ignored-by-default integration harness that requires an externally provided disposable
systemd host (for example `FLAMEWM_POLKIT_TEST_HOST` plus a prebuilt image), runs steps 1-3
above, and is executed manually in that environment. Never point it at a workstation bus: this
session's probe aborted the host daemon.

## Reproduction of the safe probes

~~~
strings /usr/lib/polkit-1/polkitd | grep -i sessionmonitor   # systemd monitor only
stat -c '%a %u %g' /usr/lib/polkit-1/polkit-agent-helper-1   # 755 0 0
unshare -Ur true                                             # uid_map EPERM
dbus-daemon --config-file=bus.conf --fork --print-pid        # private bus
DBUS_SYSTEM_BUS_ADDRESS=unix:path=.../bus.sock /usr/lib/polkit-1/polkitd -l debug
#   -> Error switching to user polkitd: Error clearing groups: Operation not permitted
~~~
