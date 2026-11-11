# Polkit authentication agent (cmp-polkit)

> Rows: `cmp-polkit` (partial, core `1951e0df`), `pol-ui`, `pol-helper`, `pol-session`, `pol-sessid`, `pol-proof` in `docs/BACKLOG.md`.

Status: core and bounded event queue landed in `flamewm-integrations-linux`; UI, shell consumer and helper I/O open. Static evidence only, no runtime proof.

## Scope check (origin/main)

- `grep -ri polkit crates/ scripts/`: no hits. Hits only in `packaging/arch/PKGBUILD:62-63`, `.SRCINFO:58-59` (optdepends on `polkit-gnome`/`polkit-kde-agent`) and `packaging/debian/control.in:9` (Suggests). FlameWM ships no agent; it relies on a third-party one.
- Reference (read-only): `.vendor/vela-main/polkit/agent/` (C++/GLib, `main.cpp`). Flow: `polkit_unix_session_new_for_process_sync` subject, `RegisterAuthenticationAgent(subject, locale, object_path)`, export `org.freedesktop.PolicyKit1.AuthenticationAgent` on that path, then prompt and answer through `polkit-agent-helper-1`.

## Protocol a minimal agent needs

| Step | Contract |
| --- | --- |
| Register | `org.freedesktop.PolicyKit1` `/org/freedesktop/PolicyKit1/Authority`, `RegisterAuthenticationAgent(subject (sa{sv}), locale s, object_path o)`. Subject: `("unix-session", {"session-id": s})` |
| Serve | Method calls on the exported path: `BeginAuthentication(action_id s, message s, icon_name s, details a{ss}, cookie s, identities a(sa{sv}))` and `CancelAuthentication(cookie s)`. Reply empty on success |
| Respond | After a password, call `AuthenticationAgentResponse2(uid u, cookie s, identity (sa{sv}))` on the Authority, or pipe cookie and password to `polkit-agent-helper-1` (setuid helper, path differs by distro) |
| Unregister | `UnregisterAuthenticationAgent(subject, object_path)` on exit |

## Owner and edges

- Owner: `flamewm-integrations-linux` (system-bus integrations; already depends on `flamewm-dbus-reactor`). New module `polkit_agent.rs`, registered in `lib.rs`.
- Bus: `flamewm_dbus_reactor::BusPump` with `on_ready_with_object_path(AGENT_PATH, ...)`. No new event source, no `dbus-tokio`, no own thread.
- No `own_name` needed: polkit calls back on the unique name.
- UI: a `AuthPrompt` trait. The window reuses the existing shell/desktop dialog pattern. Not implemented here.

## Landed in this step (non-visual core)

- `register_message`, `unregister_message`: encode the Authority calls.
- `parse_begin`, `parse_cancel`: argument validation (arity, types, empty cookie).
- `AgentSessions`: pending map keyed by cookie; duplicate rejection; cancel dismisses once; `finish` for the UI answer.
- `handle_agent_call`: routes on interface and member; returns `Reply` or `Unhandled`.
- `AuthPrompt` trait: UI boundary. `show` is fallible so a full queue becomes an error reply.
- `PolkitEventQueue`: bounded (`MAX_QUEUED_EVENTS` = 16) FIFO implementing `AuthPrompt`. The shell drains typed `PolkitEvent::{Show, Dismiss}` via `drain()`. A cancel for an undrained `Show` removes it instead of queueing a `Dismiss`. No bus, UI or shell wiring; no daemon. Tests: `full_queue_rejects_second_prompt_without_pending_entry`, `dismiss_before_drain_cancels_undelivered_show`, `dismiss_after_drain_is_queued_for_shell`.
- Tests use a mock prompt and in-process messages (no bus).

## Session-id resolution (pol-sessid, landed)

`polkit_agent::resolve_session_id(env_session_id, pid, &impl SessionLookup)` is pure:

1. `XDG_SESSION_ID` passed in by the caller, trimmed; a blank value counts as absent. Not read inside the function.
2. Otherwise `SessionLookup::session_id_for_pid(pid)`, the logind `GetSessionByPID` mapping (object path to `Id` property). Blank answer is `InvalidArgument`; lookup errors propagate. No guessed fallback.

The trait has a fake in tests only. No real logind implementation: the crate has `BusPump` with `BusKind::System` and `send_with_reply`, so a thin call is possible, but it needs a pump and a reply-timeout policy that this step does not decide. Real lookup landed: `LogindLookup<W: LogindWire>` maps `GetSessionByPID` then the `Id` property; `SystemLogind::connect()` is the blocking system-bus client (5 s timeout). `NoSessionForPID` maps to `NotFound`, other bus errors to `Unavailable`. Tests use a fake wire only (no bus). Runtime against real logind is **unverified**. Known weakness: `XDG_SESSION_ID` can be stale under `su`/`sudo` that keeps the caller's env, so env-first can name another seat. Cross-checking against logind is the follow-up once the real lookup exists.

## Session wiring decision (pol-session, design only, not wired)

Decision: **the shell process registers the agent**, on its own system-bus connection, at shell start once the session id resolves. Not the WM, not a new helper.

Reasons:

- **Reactor rule.** `flamewm-reactor` owns native event sources. The agent's bus traffic goes through `flamewm_dbus_reactor::BusPump` on the shell's existing pump (`on_ready_with_object_path(AGENT_PATH, ...)`). No second pump, no thread.
- **UI is not WM authority.** Polkit prompts are presentation; they must not live in `flamewm-wm` or `flamewm-wm-wayland`, which own windows and stacking. Registering in the WM would make a UI concern authoritative for WM lifetime.
- **Consumer locality.** `PolkitEventQueue` is drained by the shell (`drain()`). Registering in the shell keeps producer and consumer in one process, so no new IPC is needed for `Show`/`Dismiss`.
- **No new helper.** A dedicated `flamewm-polkit-agent` binary would need a new crate, new session-wrapper start order and a new IPC channel to the shell. That is more edges than the feature needs. Revisit only if the shell proves unable to stay up for the session.

When:

- After the shell's bus is ready (`BusPump::wait_ready` or the `on_ready` callback), not at process start. Failure to resolve the session id logs once and leaves the agent unregistered; no retry loop.
- Session id: `resolve_session_id(std::env::var("XDG_SESSION_ID").ok().as_deref(), std::process::id(), &logind)`. The pid is the shell's own, never a child helper.

Unregister:

- On orderly shell exit: call `unregister_message(session_id)` and send it with a bounded timeout, ignoring the error, from the shell's shutdown path.
- On crash or SIGKILL: no unregister runs. Polkit is expected to drop the agent when the registering unique name disappears, so a stale entry must not block the next shell. **Unverified:** this needs runtime proof (`pol-proof`).
- Do not implement `Drop` that sends bus traffic. Drop runs during unwinding and would block on a dead bus.

Another agent already registered:

- Do not steal the subject. A second `RegisterAuthenticationAgent` for the same session is expected to fail with a polkit error; the shell logs it once and stays unregistered. **Unverified** error name, to confirm against polkit source in `pol-proof`.
- Agents for other sessions are not our concern; the subject is per session.

Constraints for the implementation row (not done here): the shell must not hold `AgentSessions` state in its UI layer as authority; it is a pending-prompt cache that polkit's cookies key. Cancel from polkit always wins over a pending UI answer.

## Open (not in this step)

1. Prompt window: projection and HTML landed (`pol-ui` partial). `flamewm-shell/src/polkit_dialog.rs` is pure state: `PolkitEvent` to `PolkitDialog` (one active, bounded FIFO of pending, cancel returns the cookie, submit yields a wiped `Submission`). `ui/shell/polkit.html` is static with ids and `data-action` hooks. Not wired into the shell runtime and not rendered; password zeroing is a minimal overwrite (no zeroize dependency).
2. Password flow: `polkit-agent-helper-1` spawn, stdin/stdout protocol, PAM response. Security-sensitive; needs review.
3. Session wiring: which process registers (shell or desktop), when, and unregister on exit.
4. Session-id source: `XDG_SESSION_ID` vs logind `GetSessionByPID`.
5. Runtime proof: needs a host with polkit running and `pkexec` to trigger a request. None available in this step.

## Unverified

- No real polkit daemon. Mock-bus tests only. No `RegisterAuthenticationAgent` has been sent to a bus.
- Wire signatures are from the polkit spec and Vela source, not from a live capture.
