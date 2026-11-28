# p102 subsystem panic gaps under panic=abort

STATUS: OPEN

Scope: read-only audit. Source of truth: `docs/PANIC_STRATEGY.md`, `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs`, `crates/flamewm-session-core/src/diagnostics/failure.rs`, `Cargo.toml:70` (`panic = "abort"`). No crate edits, no build, no git.

## Common facts (all rows)

- Release profile is `panic = "abort"`. A panic on any thread runs the global hook (`install_panic_hook`, `failure.rs:250`), which appends a `panic` FailureRecord to the session log, then the process gets SIGABRT.
- `guard()` (`failure.rs:273`) is a pass-through in release: `catch_unwind` never returns `Err`. Its 9 production sites add no isolation.
- Supervisor (`supervision.rs`) respawns only three roles: WM (0), desktop (1), shell (2). Policy: 5 restarts per 60 s, backoff 500 ms to 8 s, then fallback plan. It logs exit code/signal per exit and writes `abort_recovery` records only on give-up or respawn failure. It does not read the panic record.
- Any role panic therefore loses that process's in-memory state. The WM has no recovery path at all.

## Table

| Subsystem | Where it runs (thread/site) | guard under abort | On panic today | Respawned | User-visible state lost | Session log |
| --- | --- | --- | --- | --- | --- | --- |
| Icon service worker | `flamewm-shell/src/icon_loader.rs:279` thread, `guard("shell-icons")` at :282 | no-op | Shell process aborts | Yes, as shell role (index 2), after backoff | Shell UI state: launcher, tray, notification center, open popups; icon cache in memory | Hook writes `panic` record; supervisor logs exit signal |
| Notification center | `flamewm-shell/src/notification_center.rs`, in-process; no dedicated thread found | n/a (no guard) | Shell process aborts | Yes, shell role | Pending and displayed notifications (in memory, not persisted here) | Hook writes `panic`; supervisor logs exit |
| Clipboard owner/keeper | `flamewm-clipboard/src/owner.rs:29`, `keeper.rs:26`; WM-side protocol in `flamewm-wm-wayland/src/protocols/clipboard.rs`; `desktop-paste` guard only covers `input.rs:62` | no-op for keeper threads (not guarded) | Owning process aborts; selection owner gone | If owner is WM: no (WM is fatal, session restarts the stack). If desktop/shell owner: yes | Clipboard contents (selection disappears when owner dies) | Hook writes `panic`; supervisor logs exit |
| Wallpaper loader | `flamewm-desktop/src/wallpaper_loader.rs:176` thread, `guard("wallpaper")` at :179 | no-op | Desktop process aborts | Yes, desktop role (index 1), then shell restarted after desktop | Desktop state: icons, selection, open dialogs; wallpaper reloads on start | Hook writes `panic`; supervisor logs exit |
| Tray | `flamewm-tray/src/item.rs` (no spawn found; runs inside shell) | n/a | Shell process aborts | Yes, shell role | Tray item list held in shell; apps must re-register StatusNotifierItem (behaviour of re-registration not verified) | Hook writes `panic`; supervisor logs exit |
| Portal (screenshot backend) | `flamewm-screenshot/src/portal.rs:602` `std::thread::spawn`, no guard | no-op / none | Screenshot process aborts; portal request fails for the caller | No. Screenshot is not a supervised role | In-flight portal request only | UNVERIFIED: screenshot binary's hook install not checked; no supervisor line |

Related, same pattern, not requested but seen: shell settings worker (`flamewm-shell/src/main.rs:871`, `guard("shell-settings")`), explorer workers (`flamewm-explorer/src/workers.rs`), audio helper (`flamewm-audio-helper/src/main.rs`).

## Findings

1. Every optional-feature guard (`wallpaper`, `shell-icons`, `shell-settings`, `desktop-paste`, `desktop-drop`, `desktop-trash`, `desktop-tool`, `frame-pacer`) is dead in release. The comments claim isolation the binary does not have.
2. A feature panic costs the whole shell or desktop process, including unrelated UI state, plus a 0.5 to 8 s restart gap.
3. Portal and other non-supervised helpers have no respawn at all.
4. The supervisor cannot tell a panic from a crash. The panic record only reaches the log through the hook, so the hook is load-bearing.
5. The WM is not recoverable in-process under either strategy; keeping it on abort is correct.

## Recommendation (abort vs unwind)

Use the table, not the guard claim, to decide.

- Keep `panic = "abort"` for the WM. Its state is not safely recoverable after a panic, and it is the only role with no fallback path. Unwind would keep a possibly corrupt compositor alive.
- Do not switch the whole workspace to unwind (Option C in `PANIC_STRATEGY.md`). It enlarges every shipped binary to protect helpers that have no supervisor anyway (portal, screenshot).
- Immediate, no build cost: make the guard docs and comments state that they are no-ops under abort, and add a release-profile assertion so the claim cannot drift silently. This is Option A and should land first.
- Decide between Option A and Option B (unwind for shell and desktop only) by measurement: binary size delta and RSS delta from `scripts/measure-panic-profile`. Before B, verify whether Cargo honours `panic` in `[profile.release.package.*]`; if not, B needs a separate `release-unwind` profile and gate-script changes.
- Independent of the decision: give the portal/screenshot helper either a supervisor role or a hook-verified log line, since it is the one row with no recovery and no log line.

## Next

- Verify the portal/screenshot binary's `install_panic_hook` call.
- Check Cargo behaviour for `panic` in `profile.<name>.package` overrides at the pinned cargo version.
- Measure Option B per the measurement section of `docs/PANIC_STRATEGY.md`.
