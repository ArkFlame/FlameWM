# Vela issues vs FlameWM

Reviewed: `vela-desktop/vela` open and recently closed issues plus merged PRs (listed via `gh`
on 2026-11-09). Feature requests and release trackers (#50, #74, #174, #198, #210, #211, #214,
#225) are not bug classes and are omitted. "unknown" means not verified by source or runtime.

Verdicts: `yes` = same defect in FlameWM, confirmed in source; `partial` = same class, different
exposure; `no` = FlameWM already handles it; `unknown` = not checked.

| Vela issue | Problem class | FlameWM | Evidence (FlameWM) | Action |
| --- | --- | --- | --- | --- |
| #89 (PR merged, "fail closed when the locker fails") | session/lock, security | yes (fixed here) | `crates/flamewm-wm-wayland/src/render/mod.rs` painted the desktop when no lock surface remained after `prune_dead_locks`; `session_lock.rs` `prune_dead_locks` refocused a window | Fixed: `session_lock.rs` `screen_for` + `State::session_locked`, `pin_focus_to_lock` in `state.rs` |
| #127 (PR merged, cancel drag on lock) | session/lock, input | yes, latent (fixed here) | `session_lock.rs` `lock()` did not clear `State::grab` (`state.rs` `Grab::Move/Resize`); `input/buttons.rs:62-80` only ends it on release | Fixed: `session_lock.rs` `enter_lock` clears `grab` |
| #159 (PR merged, hidden resize cursor during lock) | session/lock, cursor | yes (fixed here) | `state.rs` `refresh_cursor_status` lets `decor_cursor` override the client cursor; `input/pointer.rs:103-105` diverts motion while locked, so `decor_cursor` is never cleared | Fixed: `enter_lock` clears `decor_cursor`, then `refresh_cursor_status` |
| #208 / #221 (merged) settings show new value when `vela.conf` write fails | settings persistence | no | `crates/flamewm-platform/src/host/displays.rs:130-145` persists then rolls back; `host/shortcuts.rs:44-57` rolls back engine on persist error; `flamewm-settings/src/app_impl_1.rs:725` propagates `apply_settings` errors with `?` | none |
| #218 preference write failures across Settings | settings persistence | no (same evidence as #208) | `services/workspaces.rs:101-113` persists before applying, restores count on engine failure | none; re-check theme/wallpaper/clock writers if they exist outside `apply_settings` (not traced) |
| #222 session keyring follows KDE and hides GNOME login keyring | session, secrets | partial | `crates/flamewm-applications/src/browser_secrets.rs:172-176` sets `XDG_CURRENT_DESKTOP=KDE` for browser children; `crates/flamewm-session-core/src/lib.rs:312-330` picks KWallet whenever KWallet is installed | Not fixed. Needs a store-selection design (which keyring holds the logins). Do not copy Vela's approach blindly |
| #219 taskbar overflow with tray and clock | taskbar layout | unknown | Taskbar is in `crates/flamewm-shell/src/app/dock.rs`, which is out of scope for this task | Follow-up; needs a runtime check at narrow widths |
| #163 fractional-scale XWayland game resolutions | multi-monitor, HiDPI | unknown | `crates/flamewm-wm-wayland/src/presentation_scale.rs:63-66` sets fractional scale for Wayland surfaces only; no RandR mode list found in `flamewm-x11-desktop/src/topology.rs` | Follow-up: runtime check with a 1.25 scale output and a fullscreen XWayland client |
| #55 Proton fullscreen black screen (Wine backdrop) | Xwayland, fullscreen | unknown | No Wine-specific code in `crates/flamewm-wm-wayland/src` or `crates/flamewm-wm-x11/src` (grep `wine` = 0 hits) | Follow-up: runtime test with a fullscreen Wine window |
| #93 / #105 notifications and dialogs on the chosen primary monitor | multi-monitor | unknown | Only `launch_feedback.rs` and `dock.rs` matched "primary" in notification code, both out of scope | Follow-up |
| #82 `MSG_NOSIGNAL` on command replies | IPC, crashes | no (platform default) | `grep MSG_NOSIGNAL\|SIGPIPE` over `crates/` = 0 hits; Rust std ignores SIGPIPE at startup, so writes return `EPIPE` as `Err` | none; not runtime-tested |
| #49 SIGUSR1 redraw terminates session | signals, crashes | no | `grep SIGUSR1` over `crates/` = 0 hits | none |
| #57 block SIGINT/SIGTERM before GPU threads | signals, crashes | unknown | not traced | follow-up if a GPU worker thread exists |
| #147 Vulkan device loss shutdown | GPU, crashes | unknown | FlameWM renders through GLES (`GlowRenderer`); Vulkan path differs | none (different renderer) |
| #132, #156, #173, #111 SHM capture stalls and vblank misses | capture, GPU | unknown | Vela-specific SHM capture path; FlameWM capture lives in `crates/flamewm-screenshot` | none |
| #197 Ubuntu 26.10 GPU suites fail where Arch passes | tests, GPU | unknown | not traced | none |
| #175 / #181 / #184 audio volume stalls on readback | audio | unknown | not traced | none |
| #36 refresh installed apps without logout | apps | unknown | not traced | none |
| #188 dismiss hovered notification with right-click | notifications | unknown | no right-click dismiss found in `flamewm-shell` notification code | follow-up |
| #47 multi-monitor rotation | multi-monitor | unknown | rotation handling exists in `crates/flamewm-platform/src/services/display/topology.rs`; not traced | follow-up |
| #170 updater hangs and shallow mirror | updater | unknown | FlameWM has no Vela updater; not traced | none |
| #192 pointer locked by an app stays put | pointer, compositor | unknown | `input/pointer.rs:103-105` diverts motion to relative events while `locked_focus()` is set | follow-up |

## Not copied from Vela

- Vela's KDE desktop identity for all sessions (#222 root cause) is not a clear win here; FlameWM already scopes it to child processes.
- Vela's "keep last value" settings rule (#208) is already FlameWM behaviour through rollback, so no new code was added.

## Limits

- Fixes are proven by unit tests only. No runtime lock-screen run was done (no nested session driven).
- Third small confirmed bug was not found within this pass. Items marked unknown were not traced.
