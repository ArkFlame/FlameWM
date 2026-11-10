# Polkit authentication agent (cmp-polkit)

Status: core landed in `flamewm-integrations-linux`; UI and helper I/O open. Static evidence only, no runtime proof.

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
- `AuthPrompt` trait: UI boundary.
- Tests use a mock prompt and in-process messages (no bus).

## Open (not in this step)

1. Prompt window (visual; needs a design decision and an existing dialog pattern).
2. Password flow: `polkit-agent-helper-1` spawn, stdin/stdout protocol, PAM response. Security-sensitive; needs review.
3. Session wiring: which process registers (shell or desktop), when, and unregister on exit.
4. Session-id source: `XDG_SESSION_ID` vs logind `GetSessionByPID`.
5. Runtime proof: needs a host with polkit running and `pkexec` to trigger a request. None available in this step.

## Unverified

- No real polkit daemon. Mock-bus tests only. No `RegisterAuthenticationAgent` has been sent to a bus.
- Wire signatures are from the polkit spec and Vela source, not from a live capture.
