# Blocking-call audit, 2026-11-05

Scope: `flamewm-platform` (not `services/settings*`, `host.rs` settings apply), `flamewm-shell` (not startup, colour, icon loader),
`flamewm-session-core`, `flamewm-integrations-linux` (not `icon_theme`), `flamewm-lock`, `flamewm-control-dbus`.
Tests (`tests.rs`, `tests/`, `#[cfg(test)]` regions) are excluded.

Reproduce: `scripts/perf/blocking_audit.sh 40`. The script is a heuristic grep. Its `thread` and `frequency`
columns are hints marked `(verify)`; the verdicts below come from reading the callers.

## Verdicts on the top offenders

| Site | Verdict | Reason |
| --- | --- | --- |
| `crates/flamewm-shell/src/event_router.rs` `create_app_shortcut` (called on drag-release to desktop) | FIXED | Catalog scan plus `fs::copy` ran on the shell event loop. Now `spawn_app_shortcut` runs it on `flamewm-desktop-shortcut`; failure logging is unchanged. Per drop, rare. |
| `crates/flamewm-platform/src/storage.rs` `write_atomic_text` / `write_atomic_with` (fsync + rename), via `services/workspaces.rs` `persist_count` | DEFERRED | Called only when the workspace count changes, so rare. Moving it off-thread would let a reply precede durability and break restart ordering. Needs an ordered persist queue; not a small fix. |
| `crates/flamewm-lock/src/x11.rs` `grab_with_retry` (`thread::sleep` retry) | DEFERRED | Startup of the lock screen only, bounded by `timeout`. X grabs give no wake-up event; a change needs an X event source and is outside this pass. |
| `crates/flamewm-shell/src/quick_controls/supervisor.rs` `shutdown` (`kill` + `wait`) | DEFERRED | Shutdown path only. A SIGKILLed child reaps in well under 1 ms. |
| `crates/flamewm-integrations-linux/src/audio_helper.rs` `retire` (`kill` + `wait` in `Drop`) | DEFERRED | Same SIGKILL reap as above. Runs on helper replacement or drop, not per event. |
| `crates/flamewm-integrations-linux/src/backlight.rs` `write_sysfs` | DEFERRED | Single small sysfs write per brightness step. Stall is under 1 ms, not measured as a problem. |
| `crates/flamewm-session-core/src/appearance.rs` `materialize_owned_files` | DEFERRED | Session setup, once per session. |
| `crates/flamewm-integrations-linux/src/media_store.rs` `persist` (fsync) | OUT OF SCOPE | Caller is the settings or explorer family (excluded). |

## Already off the UI thread (no change)

- `removable_mounts::udisks_action` (`udisksctl` `.output()`): runs in the removable-media worker (`remount_pass`).
- `weather_provider` `CurlFetcher` (`curl` `wait`): runs in `weather_host` worker threads.
- `icon_loader`, `icon_service`: sync channels plus a UnixStream wake.
- `power.rs`, `device_watch.rs`: dedicated reader threads.
- `lock/run.rs` frame pacer (`thread::sleep`): dedicated `flamewm-lock-frames` thread.
- `audio_helper` reader thread reaps children.
- `assistant_popover::launch` and `event_router` general-settings launch use `spawn`, not `wait`.

## Ranked table (script output, top 40 raw hits)

| Rank | file:line | kind | fn | thread | frequency | expected stall | score |
| ---: | --- | --- | --- | --- | --- | --- | ---: |
| 1 | `crates/flamewm-integrations-linux/src/media_store.rs:182` | fsync | `persist` | caller thread (verify) | once or on demand (verify) | fsync: 1-50 ms, unbounded on loaded disk | 10 |
| 2 | `crates/flamewm-integrations-linux/src/media_store.rs:193` | fsync | `persist` | caller thread (verify) | once or on demand (verify) | fsync: 1-50 ms, unbounded on loaded disk | 10 |
| 3 | `crates/flamewm-platform/src/storage.rs:26` | fsync | `write_atomic_text` | caller thread (verify) | once or on demand (verify) | fsync: 1-50 ms, unbounded on loaded disk | 10 |
| 4 | `crates/flamewm-platform/src/storage.rs:78` | fsync | `write_atomic_with` | caller thread (verify) | once or on demand (verify) | fsync: 1-50 ms, unbounded on loaded disk | 10 |
| 5 | `crates/flamewm-shell/src/quick_controls/supervisor.rs:312` | child-wait | `shutdown` | caller thread (verify) | once or on demand (verify) | process spawn+wait: 5-500 ms | 10 |
| 6 | `crates/flamewm-shell/src/weather_provider.rs:233` | child-wait | `get` | caller thread (verify) | once or on demand (verify) | process spawn+wait: 5-500 ms | 10 |
| 7 | `crates/flamewm-shell/src/weather_provider.rs:240` | child-wait | `get` | caller thread (verify) | once or on demand (verify) | process spawn+wait: 5-500 ms | 10 |
| 8 | `crates/flamewm-integrations-linux/src/backlight.rs:199` | fs-io | `read_text` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 9 | `crates/flamewm-integrations-linux/src/backlight.rs:210` | fs-io | `write_sysfs` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 10 | `crates/flamewm-integrations-linux/src/media_store.rs:174` | fs-io | `persist` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 11 | `crates/flamewm-integrations-linux/src/media_store.rs:185` | fs-io | `persist` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 12 | `crates/flamewm-integrations-linux/src/media_store.rs:190` | fs-io | `persist` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 13 | `crates/flamewm-integrations-linux/src/media_store.rs:242` | fs-io | `gc` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 14 | `crates/flamewm-integrations-linux/src/media_store.rs:257` | fs-io | `existing_file_matches` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 15 | `crates/flamewm-integrations-linux/src/media_store.rs:298` | fs-io | `open_source` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 16 | `crates/flamewm-integrations-linux/src/power.rs:139` | fs-io | `read_cpu_ticks` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 17 | `crates/flamewm-integrations-linux/src/themes/import.rs:117` | fs-io | `install_theme` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 18 | `crates/flamewm-integrations-linux/src/themes/import.rs:194` | fs-io | `remove_entry` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 19 | `crates/flamewm-integrations-linux/src/themes/import.rs:209` | fs-io | `commit` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 20 | `crates/flamewm-integrations-linux/src/themes/import.rs:220` | fs-io | `commit` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 21 | `crates/flamewm-integrations-linux/src/themes/import.rs:221` | fs-io | `commit` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 22 | `crates/flamewm-integrations-linux/src/themes/import.rs:222` | fs-io | `commit` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 23 | `crates/flamewm-integrations-linux/src/themes/import.rs:245` | fs-io | `write_plan` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 24 | `crates/flamewm-integrations-linux/src/themes/import.rs:248` | fs-io | `write_plan` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 25 | `crates/flamewm-integrations-linux/src/themes/import.rs:269` | fs-io | `load_source` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 26 | `crates/flamewm-integrations-linux/src/themes/import.rs:275` | fs-io | `load_source` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 27 | `crates/flamewm-integrations-linux/src/themes/mod.rs:93` | fs-io | `discover` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 28 | `crates/flamewm-lock/src/document.rs:594` | fs-io | `decode_file` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 29 | `crates/flamewm-lock/src/session.rs:123` | fs-io | `username` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 30 | `crates/flamewm-lock/src/session.rs:127` | fs-io | `username` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 31 | `crates/flamewm-platform/src/host.rs:81` | fs-io | `start` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 32 | `crates/flamewm-platform/src/services/workspaces.rs:25` | fs-io | `load_persisted_count` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 33 | `crates/flamewm-platform/src/storage.rs:17` | fs-io | `write_atomic_text` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 34 | `crates/flamewm-platform/src/storage.rs:25` | fs-io | `write_atomic_text` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 35 | `crates/flamewm-platform/src/storage.rs:80` | fs-io | `write_atomic_with` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 36 | `crates/flamewm-platform/src/storage.rs:83` | fs-io | `write_atomic_with` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 37 | `crates/flamewm-session-core/src/appearance.rs:176` | fs-io | `breeze_color_scheme_file` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 38 | `crates/flamewm-session-core/src/appearance.rs:331` | fs-io | `materialize_owned_files` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 39 | `crates/flamewm-session-core/src/appearance.rs:339` | fs-io | `materialize_owned_files` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |
| 40 | `crates/flamewm-session-core/src/appearance.rs:341` | fs-io | `materialize_owned_files` | caller thread (verify) | once or on demand (verify) | syscalls: <1 ms small files, grows with size | 6 |

## Not covered

- No systematic check for `std::sync::Mutex` held across I/O. Only the `icon_service` and `pulse` locks were read.
- D-Bus: no blocking `zbus::blocking` / `dbus` call was found in the owned crates. Not exhaustively traced.
- Event-loop stall was not measured before or after the change. Only the new worker's logic is tested.
