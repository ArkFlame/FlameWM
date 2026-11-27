# Unit p38-settings-app-idle
STATUS: DONE

Crates flamewm-settings and flamewm-settings-core: when the window is closed or hidden there must be zero timers/polls; audit and fix.

## Result

Audit only: no production edit was needed because both crates are already exclusively event/fd
driven when the window is closed or hidden. Investigation used rg/Read-style source inspection
(no `git`, `cargo`, build, test, Python or sleep loop). Reference-first: `.vendor/smithay-master/src/wayland/idle_notify/mod.rs:59-62` uses one-shot, activity-rearmed
`TimeoutAction::Drop` sources, and `.vendor/kwin-master/src` was listed; the same precedent was
recorded in p29/p30. Reading a reference never made it a dependency or runtime input.

Every row below is **UNBUILT** (static evidence only; the orchestrator builds and tests after the
batch lands).

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-settings/src/runtime.rs:21-128` | The window loop is `run_with_controller_events_role_and_two_readable_fds` with exactly two readable sources (session-bus `signal_fd`, wallpaper `wallpaper_fd`). `poll_background` runs only inside a controller-event or wake callback; there is no timer registration, no interval and no per-frame poll. Closing the window ends the loop, so both sources disappear with it. | NONE — already zero timers/polls; UNBUILT. | None. |
| `crates/flamewm-settings/src/control.rs:28-51` | `SettingsSignals` owns one receive-only D-Bus descriptor; `drain` only queues typed `SettingsWake`s. The source comment states "No timer or per-frame polling is involved." No wake is manufactured while idle or hidden. | NONE — event-driven fd; UNBUILT. | None. |
| `crates/flamewm-settings/src/runtime.rs:94-101`, `crates/flamewm-settings/src/runtime.rs:182-185` | Visibility is handled on the controller turn: `set_obscured` records hidden state, reprojections refresh authoritative snapshots but skip `settle` while hidden, and the uncover turn settles once. Hidden windows therefore receive no extra redraw or wake. | NONE — no new wake source; UNBUILT. | None; visuals unchanged. |
| `crates/flamewm-settings/src/app_impl_2.rs:28-69`, `:530-545` | `poll_theme_jobs` / `poll_open_uri` use `try_recv` only when a real event or wake arrives; with no job in flight both return immediately with no allocation or control traffic. They are completion consumers, not polling sources. | NONE — removing them would delay real completions; UNBUILT. | None. |
| `crates/flamewm-settings/src/file_chooser.rs:76-133`, `crates/flamewm-settings/src/theme_picker.rs:399-421` | Worker threads publish results through the wallpaper wake fd (or a completion channel consumed on a wake). No production loop sleeps or re-arms while the window is hidden. | NONE — fd/channel driven; UNBUILT. | None. |
| `crates/flamewm-settings/src/xdg_cache.rs:34-100`, `crates/flamewm-settings-core/src/xdg_applications.rs:360-424` | `XdgWatch`/`XdgStamp` are a prebuilt path list plus `stat`-based identity, not an inotify watcher or a poller. Stamping happens only inside a repaint that a real event/wake triggered. | NONE — no periodic rescan exists; UNBUILT. | None. |
| `crates/flamewm-settings-core/src/lib.rs` (752 lines), `crates/flamewm-settings-core/src/night_light.rs`, `crates/flamewm-settings-core/src/xdg_startup.rs` | No `Duration`, `Instant`, `sleep`, `thread`, timer or polling loop occurs in this crate's production code; the only match in `lib.rs` is an import, and night-light types are pure schedule data. | NONE — no timer to remove; UNBUILT. | None. |
| `crates/flamewm-settings/src/render_backend_row.rs:60-64` | The only production `sleep` is a 20 ms bounded wait inside a 10 s deadline, reached solely by an explicit render-backend probe action. It is not registered with the reactor and cannot fire while hidden. | NONE — user-action scope, not idle; UNBUILT. | None. |
| `crates/flamewm-settings/src/consts.rs:426-429`, `crates/flamewm-settings/src/actions.rs:373-382` | The `OnceLock<Instant>` only timestamps slider-drag coalescing, and the spawned waiter thread blocks on the assistant child during a voice-setup action. Neither is a timer or idle poll. | NONE — no change; UNBUILT. | None. |
| `.vendor/smithay-master/src/wayland/idle_notify/mod.rs:59-62` (reference only) | Upstream precedent re-arms idle timeouts from activity events and drops them otherwise; no unconditional idle poll. Confirms the local event-driven design already satisfies the requirement. | NONE — reference evidence only; UNBUILT. | None; no runtime dependency added. |
| `docs/swarm/pool/p38-settings-app-idle.md:1-4` | Unit file had `STATUS: OPEN` and a bare `STARTED` marker. | Replaced status with `DONE` and the marker with this inventory, row table and verdict — documentation only; UNBUILT. | None. |

## Verification

No compiler, test runner, `git`, Python or sleep loop was invoked, so no runtime measurement backs
this result; every entry is static source evidence marked UNBUILT. No production file was changed,
therefore no gate was loosened and no pixel can move. The only edit is this unit file.

PASS — the audit covers every timer/poll-shaped path in `flamewm-settings` and
`flamewm-settings-core`: the two crates register no timers, intervals or per-frame polls, the
settings window runs on two reactor-readable descriptors plus typed wakes, and completion consumers
only read channels when an event or wake already arrived. Closed and hidden windows therefore hold
zero timers/polls already, so no production fix was applied and visuals remain pixel-identical;
with no code change there is nothing for the orchestrator's build to break, though the static-only
evidence and UNBUILT rows remain as stated.
