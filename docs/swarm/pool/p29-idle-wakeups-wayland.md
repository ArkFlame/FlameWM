# Unit p29-idle-wakeups-wayland
STATUS: DONE

Crate flamewm-wm-wayland: find timers and polling loops that wake when nothing changed (rg 'Timer|interval|sleep|poll'); make them event driven or coalesced, visuals identical. Report share estimate per source.

## Result

Investigated with rg/Read only (no builds/tests, per brief). Reference-first: .vendor/smithay-master/src/wayland/idle_notify/mod.rs re-arms idle timeouts from notify_activity on input events, and .vendor/kwin-master was checked for idle handling; precedent is one-shot, event-rearmed timers with no unconditional polling, which matches this crate.

Production edits: 0. Every timer found is already one-shot (TimeoutAction::Drop) or re-armed only on a real event / behind an opt-in env var, and the loop blocks in run(None, ...). A cosmetic edit would risk loosening a gate or changing visuals, so none was made. All rows below are UNBUILT (no edit applied; builds forbidden by the brief).

| file:line | finding | change | risk |
|---|---|---|---|
| crates/flamewm-wm-wayland/src/main.rs:1004 | event loop calls run(None, ...), blocking on FD sources; no always-on poll loop | none - already event driven; UNBUILT | none |
| crates/flamewm-wm-wayland/src/profile.rs:84-106 | 1 Hz compose dump only with FLAMEWM_PROFILE_COMPOSE=1 and profiler enabled; self-rearming diagnostic | none - opt-in gate; UNBUILT | none by default; 1 tick/s when opted in |
| crates/flamewm-wm-wayland/src/main.rs:898-914 | dump retry 500/250 ms only under FLAMEWM_WAYLAND_DUMP; stops on state.dumped | none - env-gated one-shot chain; UNBUILT | none |
| crates/flamewm-wm-wayland/src/main.rs:938-967 | script-drag timer only under FLAMEWM_WAYLAND_SCRIPT_DRAG; drops after steps | none - env-gated; UNBUILT | none |
| crates/flamewm-wm-wayland/src/main.rs:978-997 | content deadline 15 s, one-shot, armed only with autostart clients | none - one-shot; UNBUILT | none |
| crates/flamewm-wm-wayland/src/state/redraw.rs:144-159 | 16 ms winit pacing coalesced by winit_frame_armed, armed only on a redraw request | none - coalesced; UNBUILT | none |
| crates/flamewm-wm-wayland/src/drm/mod.rs:435-448 | FRAME_RETRY 16 ms only when a retry is needed, guarded by retry_scheduled | none - event-driven; UNBUILT | none |
| crates/flamewm-wm-wayland/src/drm/mod.rs:648-660 | first-frame deadline 8 s startup one-shot, guarded by first_frame_deadline | none - one-shot; UNBUILT | none |
| crates/flamewm-wm-wayland/src/drm/init.rs:207-226 | hotplug settle 250 ms one-shot per udev burst, gated by burst.event() | none - one-shot; UNBUILT | none |
| crates/flamewm-wm-wayland/src/render/backdrop.rs:569-576 | desktop recapture wake one-shot, desktop_wake_armed guard | none - coalesced; UNBUILT | none |
| crates/flamewm-wm-wayland/src/protocols/clipboard.rs:329-337 | READ_TIMEOUT 2 s one-shot per selection, removed on completion | none - one-shot; UNBUILT | none |
| crates/flamewm-wm-wayland/src/state/process.rs:32-51 | pidfd READ interest; code states nothing polls for exits | none - FD driven; UNBUILT | none |

Share estimate per source (default config, steady idle): timers/polling ~0% of wakeups; ~100% are FD/event driven (wl_display, winit, udev/logind, D-Bus/control, XWayland property watcher, audio ping). With FLAMEWM_PROFILE_COMPOSE=1 the 1 Hz compose dump is 100% of timer-driven wakeups (1/s). With FLAMEWM_WAYLAND_DUMP there are at most 2 retries per dump. With FLAMEWM_WAYLAND_SCRIPT_DRAG there are ~steps wakeups over total_ms, then the timer drops.

PASS - investigation complete; flamewm-wm-wayland is already event driven/coalesced with no idle wakeups to remove, so 0 production edits were made and visuals are unchanged. Findings are static inspection only: builds and tests were forbidden by the brief and every row is marked UNBUILT; the only residual risk is that nothing was compiled, but no code was changed.
