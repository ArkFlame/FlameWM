# Vela gap plan: shared accent, quick settings, notification centre and DND

Snapshot: 2026-11-08. Scope: gaps 6, 7 and 10 from `docs/competitors/vela/FEATURES.md` section 5 and `DESIGN_COMPARISON.md` section 8.

Directive: do not risk performance or memory. Blur, surface and outline look stays. Every item must keep its RSS/PSS budget. Items that need resident GPU buffers or live previews are rejected unless the buffer is built lazily on open and freed on close.

## Baseline and budget rules

- Baseline, FlameWM idle session mode: 23.8 MiB PSS, 57.4 MiB RSS (`README.md` Performance, measured 2026-10-30, nested Xephyr 1280x800, FlameWM processes only, no apps). Not re-measured this session. Host load was about 28.
- Budget rule for every item:
  - Idle delta: 0 MiB PSS and 0 MiB RSS. No new resident process, thread, cache or GPU buffer.
  - Transient delta while a popup is open: at most 2 MiB PSS above idle, freed on close. This is a proposed limit, not yet measured.
- Measurement before and after each item: `tools/bench_mem.sh idle` on a release build (`scripts/build-deb` or `cargo build --release`). Debug builds are not evidence (`README.md` Build). Until that run exists, the delta is `unmeasured`.

## Current state (verified in source, 2026-11-08)

| Gap | What exists | What is missing |
| --- | --- | --- |
| 10. Shared accent | `flamewm_api::settings::DEFAULT_ACCENT_RGB` (`crates/flamewm-wm-wayland/src/snap_preview.rs:58`). Shared settings accent is read by snap preview (`snap_preview.rs:69-71`). `crates/flamewm-ui-core/src/surface.rs:717-730` tests neutral surfaces for each accent. About 100 files mention "accent". | Per-surface consumption is not proven for taskbar, dock, launcher and widgets. The screenshots show blue on the workspace grid, orange on the memory ring and blue on the CPU ring. |
| 6. Quick settings | `crates/flamewm-shell/src/quick_controls/` (host, protocol, popup, supervisor). The supervisor spawns the host lazily on first use (`quick_controls/supervisor.rs:108-110`, `:141-153`, `spawn_host` at `:91`). Wi-Fi: about 42 files reference NetworkManager or wifi. MPRIS: 11 files. Night light: `crates/flamewm-settings-core/src/night_light.rs`, gamma in `crates/flamewm-wm-wayland/src/protocols/gamma.rs`. | No single sheet with tiles. No symbol named "quick settings" in `crates/`. Bluetooth: 1 file. |
| 7. Notification centre and DND | Centre view model: `crates/flamewm-shell/src/notification_center.rs` (grouping, slots, outside-press close, escape close). DND: `crates/flamewm-integrations-linux/src/notification_history.rs:41,49,62,84-85` (`DoNotDisturb` suppression, critical bypass). | Centre-to-DND toggle UI not confirmed. Notification server on D-Bus not confirmed (`Notify` hits are in senders: clipboard, screenshot, desktop widgets). History cap and persistence not confirmed. |

## Prioritised plan

### P1. Shared accent token (gap 10). Size: S. Risk: none to memory.

- Owner: `crates/flamewm-ui-core` (token and surface helpers). Source of value: `crates/flamewm-api` settings (`DEFAULT_ACCENT_RGB`).
- Work:
  1. Audit every active-state colour constant in taskbar, dock, launcher, widgets and quick controls. List them with path:line. Expected: hard-coded blue in several places.
  2. Route every active-state colour through one accent accessor in `flamewm-ui-core`.
  3. Keep the glass tint, blur radius and outline untouched. Only the accent channel changes.
- Memory delta: 0. Colour values are constants. No new buffers.
- Acceptance: unit test that each active-state surface reads the accessor (extend `surface.rs` tests near line 717). Runtime check: `tools/flamewm_visual_tests.py` for two accent values, with screenshots compared. Runtime claim needs fresh evidence (`flamewm-runtime-evidence`).
- Blocker: none known.

### P2. Notification centre DND toggle and history bounds (gap 7). Size: S to M. Risk: low.

- Owner: `crates/flamewm-integrations-linux/src/notification_history.rs` (DND state and suppression). UI: `crates/flamewm-shell/src/notification_center.rs` and the centre host.
- Work:
  1. Confirm the D-Bus `org.freedesktop.Notifications` server exists. If it does not, this item becomes L and must be split out of this plan.
  2. Expose the existing DND flag as a toggle in the centre header. Reuse `outside_press_closes` and `is_escape_key` (`notification_center.rs:23-39`).
  3. Bound history to a fixed entry count and store text only. Thumbnails and icons are loaded lazily on open and dropped on close.
- Memory delta: 0 at idle. Transient: text for at most the bounded history, plus icons loaded on open. Must stay under the 2 MiB transient limit. Enforce the cap in code, not by convention.
- Acceptance: unit test for the cap and for critical-bypass (`notification_history.rs:84-85`). Runtime: send three notifications with DND on and off, verify popup suppression and centre list.

### P3. Quick-settings sheet (gap 6). Size: M. Risk: medium (new popup surface). Gate on P1.

- Owner: `crates/flamewm-shell/src/quick_controls/` (host and popup). Tile data from `crates/flamewm-integrations-linux` (network, audio, MPRIS), `crates/flamewm-settings-core/src/night_light.rs`, and the DND flag from P2.
- Tiles, in priority order: volume, brightness, night light, Wi-Fi (read-only status first), DND (from P2). Bluetooth is out of scope until a daemon exists (1 hit today).
- Work:
  1. Reuse the existing quick-controls host. Do not add a second process.
  2. Keep the lazy spawn in `supervisor.rs:141-153`. The host must start on first open, not at shell start.
  3. Confirm the host exits or sleeps after close. If it does not, add an idle exit. This is required by the directive, because a resident host would add RSS at idle.
  4. Tiles render from the shared accent (P1). No new blur layers.
- Memory delta: idle 0 MiB. Open: at most 2 MiB PSS transient, measured with `tools/bench_mem.sh idle` before and after opening the sheet. The measurement method must be added to the acceptance record.
- Acceptance: host spawn count is 1 per open and 0 per idle (add a debug counter or log line). Runtime check for each tile. Runtime claim needs fresh evidence.

## Rejected items (memory or GPU risk)

| Item | Reason | Condition to revisit |
| --- | --- | --- |
| Resident window preview textures for Alt+Tab and Task View (gap 4, 5) | Keeps one GPU or shared-memory buffer per window at idle. Breaks the idle budget with many windows open. | Only if captured lazily on open and freed on close, with a measured transient peak under 2 MiB per preview set. Separate plan. |
| Always-running quick-controls host | Adds a resident process at idle. | Not accepted. Lazy spawn only. |
| Resident media thumbnail cache | Grows with library size. | Not accepted in this plan. Lazy load only. |
| Extra blur layers for the quick sheet | Adds a GPU pass per frame while open. The directive keeps the existing glass look. | Reuse the existing glass surface. |

## Order and gates

1. P1 first. It unblocks P3 tile colours and has no memory cost.
2. P2 next. It provides the DND tile for P3.
3. P3 last. It is the only item that adds a popup surface.

Every item needs `./scripts/check` (build and native checks) and `./scripts/verify-visual` for the visual claim. Runtime and performance claims need the fresh evidence above.

## Not checked in this plan

- The D-Bus notification server is confirmed (checked 2026-11-09): `crates/flamewm-integrations-linux/src/notifications.rs:431` answers `GetCapabilities` and `:450` answers `Notify`. P2 step 1 is resolved. The DND toggle is wired end to end (checked 2026-11-09): `set_dnd` at `notifications.rs:178` is called from `crates/flamewm-shell/src/main.rs` (`Command::ToggleDnd`), and the UI button is `ui/shell/notification-center.html` (`notification.center.dnd`). P2 step 2 is resolved.
- Per-surface accent consumption is not proven. P1 starts with that audit.
- No release build was measured this session. All deltas are `unmeasured`.

## Bounded browser keyring audit (2026-11-10)

- Static source audit: browser secret policy in `crates/flamewm-applications/src/browser_secrets.rs` sets `XDG_CURRENT_DESKTOP=KDE`, `XDG_SESSION_DESKTOP=KDE`, and `KDE_SESSION_VERSION` only on a browser child process when the selected KWallet backend is active. FlameWM's own session environment is not changed. Both browser launch paths call the policy; explicit `basic`, `gnome-libsecret`, and `none` flags are preserved without the KDE override. Helium is untouched by default; only its explicit KWallet opt-in enables the override.
- Existing tests cover explicit KWallet and basic/GNOME-libsecret choices. This audit found no narrow, proven fix to make. No GNOME keyring daemon, wallet, or secret was queried or modified; no live host compatibility claim is made. This is static evidence only; end-to-end browser/KWallet and GNOME Secret Service behavior remains unverified.
