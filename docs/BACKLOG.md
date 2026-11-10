# Backlog index

Single ranked index of open candidates and recent landings. Detail lives in `docs/backlog/*.md` and `docs/competitors/**`. Snapshot: origin/main `1951e0df`, 2026-11-10. Done rows cite the landing sha, checked as an ancestor of origin/main. Unknown or unverified status stays `open`. Static evidence only unless a row says otherwise. Rank = order within the table; open rows first.

Ids: `read-*` = docs/backlog/read.md, `meas-*` = measure.md, `wrss-*` = wayland-rss.md, `look-*` = look.md, `dock-*` = dock-translucency.md, `cmp-*` = compare.md, `gap-*` = docs/competitors/vela/GAP_PLAN.md, `bt-*` = bluetooth.md, `pol-*` = polkit.md, `break-*` = break.md, `wsh-*` = wayland-shared.md.

| id | area | title | evidence | impact | size | status |
| --- | --- | --- | --- | --- | --- | --- |
| meas-blur | perf | Full-frame CPU blur dominates the frame | `docs/backlog/measure.md:44`: blurs 77-83 ms of 83-93 ms batch | up to ~80 ms/frame (estimate) | L | open |
| meas-settings | perf | Settings app startup outlier | `docs/backlog/measure.md:45`: 1774 / 1844 ms mapped / painted; cold after shadow cache 463 ms painted median (`measure.md:62-70`) | up to ~1.5 s per open (estimate) | M | open |
| meas-terminal | perf | Terminal paints ~340 ms after map | `docs/backlog/measure.md:47`: 276 / 618 ms | up to ~340 ms per open (estimate) | M | open |
| meas-heap | perf | Idle RSS: 40 MB heap/anon unattributed | `wayland-rss.md (line unpinned)`; no heap profiler run | unknown until profiled | M | open |
| wrss-scratch | perf | Drop scratch texture after backdrop capture | `wayland-rss.md (line unpinned)`: -3.0 to -3.5 MB PSS, measured | below 5 MB bar | S | open |
| wrss-gpu | perf | Re-measure idle RSS on a GPU host | `wayland-rss.md (line unpinned)`: all numbers from llvmpipe | unknown | S | open |
| meas-build | quality | `syn` and `x11rb-protocol` single-unit build cost | `docs/backlog/measure.md:43-50`: 50-110 s units | build time, gain unmeasured | M | open |
| meas-sign | quality | Harness `measure.py` inverts signed `translate_coords` x/y | `docs/backlog/wayland-shared.md:56,71` | wrong measured frame origin | S | open |
| meas-perf-blocked | quality | `perf record` blocked: `perf_event_paranoid=4` | `docs/backlog/measure.md:69` | no CPU profile of cold settings paint | S | open |
| read-E1 | reliability | Icon-theme rollback result dropped (data-loss path) | `read.md:36`; pattern gone in `themes/import.rs` | would lose user icon theme | S | done e96156bc |
| read-E2 | reliability | Removable-mount save and auto-mount failures silent | `read.md:37`; no `let _ =` hits in `removable_mounts.rs` | silent mount failure | S | done e96156bc |
| read-E3 | reliability | Desktop settings sync `on_ready` error dropped | `read.md:38`; `desktop/src/main.rs:567` now binds `ready` | stale settings UI | S | done e96156bc |
| read-P1 | reliability | `unreachable!()` on D-Bus string-array input | `read.md:16`; `control-dbus/src/lib.rs:482` now `from_item` with `?` | desktop/shell panic | S | done e96156bc |
| read-E4 | reliability | Icon submit error dropped when queue full | `desktop/src/projection.rs:418` `let _ = self.service.submit(` | missing icons, no log | S | open |
| read-E5 | reliability | Empty `Err(_) => {}` arms in icon loader | `shell/src/icon_loader.rs:269,316,335` | unknown until read | S | open |
| read-E6 | reliability | Lock session settings `connect().ok()` falls back silently | `lock/src/session.rs:36,79` | silent default settings | S | open |
| read-P2 | reliability | `expect` on client compositor data | `wm-wayland/src/state.rs:1132` | compositor panic, hard to reach | S | open |
| read-P3 | reliability | `expect` in XWayland callback after teardown | `wm-wayland/src/xwayland.rs:690` (per read.md) | compositor panic, timing unverified | M | open |
| dock-wl | visual | Dock translucency on Wayland not runtime-verified | `dock-translucency.md:32,63`; `docs/DOCK_BACKDROP.md:109` | visual regression risk | M | open |
| dock-x11 | visual | Dock pill alpha follows opacity; backdrop reinstated after hide (X11) | `dock-translucency.md:31,51-56` 5-run table | dark pill over bright window (fixed on X11) | M | done 7aa3156e, 93448de1 |
| dock-wire | reliability | Dock surface percent keys decoded as Integer | `dock-translucency.md:38`; test `control-wire/src/tests.rs:991` | wrong dock opacity | S | done 32f28041 |
| look-muted | visual | `--muted` still differs in 4 stylesheets: `desktop.css`, `task-manager.css`, `shell/flamewm.css`, showcase `ui/flamewm.css` (terminal and explorer deliberate) | `docs/backlog/look.md:39`; assistant, osd, archiver removed in `0de670a1` | colour drift | S | open (base landed, see look-muted-done) |
| look-muted-done | visual | One canonical `--muted` in `ui/base/base.css` | `look.md:7-12`; `ui/base/base.css:3` | colour consistency | S | done bc8098cd |
| look-radius | visual | Two radius scales in `ui/base/tokens.css` | `docs/backlog/look.md:35`; radius pass dropped | token drift | M | open |
| look-radius-large | visual | 14-28 px radii on cards outside scale | `docs/backlog/look.md:29-34` (design decisions, not defects) | decision needed | S | open |
| look-harness | quality | Archiver blank, assistant black, task-manager never settles in harness | `docs/backlog/look.md:45-48` | blocks visual verification | M | open |
| gap-accent | visual | Shared accent token: route active colours through one accessor | `docs/competitors/vela/GAP_PLAN.md` P1; `flamewm-wm-wayland/src/snap_preview.rs:58` | consistency | S | open |
| cmp-quick | feature | Quick settings in one surface (Wi-Fi, BT, power, brightness) | `cmp-2`; `shell/src/quick_controls/host/run.rs` exists | partial | M | open |
| cmp-power | feature | Power profile switch (UPower PowerProfiles) | `integrations-linux/src/power_profiles.rs`; facade `bd8c55ba`, plumbing `4021f35f` | UI toggle/tile not verified | M | done bd8c55ba, 4021f35f |
| cmp-dnd | feature | Do Not Disturb toggle | `integrations-linux/src/notifications.rs:178` `set_dnd`; `shell/src/main.rs:280` | stale GAP_PLAN:77 claim | S | done (pre-existing, no sha) |
| meas-tm | reliability | Task-manager startup probe never paints | `docs/backlog/measure.md:43` (3/3 `painted` None) | fixed by probe change | S | done c759a2a8 (not re-measured) |
| meas-shadow | perf | Text-shadow image cache for cold settings paint | `docs/backlog/measure.md:62-70`; `819a2284` | cold settings painted median 463 ms baseline | S | done 819a2284 |
| meas-blur-eq | quality | Blur 2x tier threshold recalibrated (`T_BLUR` 21) | `852f81ce`; `edd3d8ca` tiers; `docs/backlog/measure.md` | render drift bound set | S | done 852f81ce |
| cmp-wm-session-lock | quality | README says "No session lock"; code has it | `README.md:89`; `wm-wayland/src/session_lock.rs` | doc drift | S | done (README limitation corrected; runtime proof still open) |
| cmp-bt | feature | Bluetooth pairing and in-shell prompts | `cmp-3`; Vela `bluetooth/` 25 files; facade `b6bddef9` on origin/main | partial: device-control facade only; `bt-*` open | L | partial |
| bt-reply | feature | Drain and track BlueZ replies (serials, errors) through `on_ready` | `docs/backlog/bluetooth.md:21` | replies queue unread on facade connection | S | open |
| bt-plumb | feature | `BluetoothAction`, control-wire, platform routing, Pair/Trust/Remove control actions (mirror `4021f35f`) | `docs/backlog/bluetooth.md:22` | no control path to facade | M | open |
| bt-agent | feature | `org.bluez.Agent1` registration for PIN/passkey prompts (security-sensitive) | `docs/backlog/bluetooth.md:23` | Pair fails for most devices | L | open |
| bt-list | feature | Device list and adapter `Powered` state (`GetManagedObjects`, `PropertiesChanged`) | `docs/backlog/bluetooth.md:24` | no device list | M | open |
| bt-ui | feature | Quick-controls Bluetooth row | `docs/backlog/bluetooth.md:25` | no UI | S | open |
| cmp-polkit | feature | Polkit authentication agent | `cmp-4`; core `1951e0df` on origin/main; `docs/backlog/polkit.md:26-34` | partial: core landed; `pol-*` open | L | partial |
| pol-ui | feature | Polkit prompt window (needs design decision and dialog pattern) | `docs/backlog/polkit.md:37` | no prompt | M | open |
| pol-helper | security | Password flow via `polkit-agent-helper-1` (stdin/stdout, PAM response) | `docs/backlog/polkit.md:38` | no auth possible | L | open |
| pol-session | feature | Session wiring: which process registers, when, unregister on exit | `docs/backlog/polkit.md:39` | agent not registered | M | open |
| pol-sessid | feature | Session-id source: `XDG_SESSION_ID` vs logind `GetSessionByPID` | `docs/backlog/polkit.md:40` | unresolved | S | open |
| pol-proof | quality | Runtime proof with polkit daemon and `pkexec` | `docs/backlog/polkit.md:41` | mock-bus tests only | M | open |
| cmp-a11y | feature | Magnifier, colour filters (sticky keys partly: `26020113`) | `cmp-5` | absent | L | open |
| cmp-alttab | feature | Alt+Tab live previews | `cmp-6`; no preview capture found | unverified | L | open |
| cmp-volume | feature | Per-app volume wheel on taskbar button | `cmp-7`; `integrations-linux/src/pulse.rs:180` backend exists | partial | S | open |
| read-P4 | quality | `unreachable!()` in MPRIS (provably unreachable) | `integrations-linux/src/mpris.rs:575` | cosmetic | S | open |
| read-P5 | quality | `pub` test oracle with `expect` in production file | `control-dbus/src/mutation_queue.rs:196` | limited | S | open |
| read-P6 | quality | `expect` on thread spawn and socket pair in icon paths | `integrations-linux/src/icon_service.rs:261,617`; `shell/src/icon_loader.rs` | startup only | M | open |
| read-K1 | quality | Reads undefined setting key `launchFeedbackAnimation` | `shell/src/launch_feedback.rs:328`; no row in `platform/src/settings/stable.rs` | flag cannot be enabled | S | open |
| read-D1 | quality | Dead `registered_items` | `tray/src/watcher.rs:114` | dead code | S | open |
| read-D2 | quality | Dead `runtime_document` | `ui-document/src/template.rs:94` | dead code | S | open |
| read-D3 | quality | `icon_chain` used only by test | `integrations-linux/src/themes/mod.rs:132` | test-only API | S | open |
| read-D4 | quality | `flush_settings` used only by test | `platform/src/host.rs:348` (confirm first) | possibly dead | S | open |
| read-D5 | quality | Unused constants in glass, palette, recipes | `read.md:67` | dead code | S | open |
| read-D6 | quality | Dead `set_ring_mask` (dock, other owner) | `render-x11/src/surface_controller/dock.rs:153` | dead code | S | open |
| read-D7 | quality | Unused `titlebar_rect`, `SESSION_LOCK_INTERFACE` | `skin/src/recipes/window_chrome.rs:82`; `wm-wayland/src/lock_global.rs:13` | dead code | S | open |
| read-U1 | quality | Settings accessor duplicated in 5 places | `read.md:94` | maintenance | M | open |
| read-U2 | quality | `node_backdrop_clip` and `scale_rect` duplicated in render-x11 (covers U3) | `read.md:95-281`; render-x11 copies now re-export `flamewm-render-core::lower` (871b4d33) | maintenance | S | done (871b4d33) |
| dup-rect-test | quality | `fn rect(x,y,w,h) -> Rect` test helper copied in 5 render-x11 test modules | `render-x11/src/app/clip.rs`, `damage_plan.rs`, `render.rs`, `native/coverage.rs` tests; `app/primitives.rs` | test maintenance | S | done (4 copies in render-x11 tests; primitives.rs had none; added Rect::new in render-core) |
| dup-wall-clock | quality | `wall_clock_delay_ms` duplicated in shell-core and reactor | `shell-core/src/clock.rs:40`; `reactor/src/lib.rs:337` (shell-core comment says it stays dependency-free) | needs dep-edge decision | S | open |
| dup-build-rerun | quality | `rerun_tree` and `stage_tree` copied in desktop, settings and shell `build.rs` | moved to `crates/flamewm-render-compiler/src/build_tree.rs` (existing build-dep of all three; no new edge) | done | M | done |
| dup-color-rgb | quality | `rgb` helper in terminal and explorer; `send_state` in x11-desktop and wm-x11; `runtime_image` in desktop and shell | `terminal/src/ui.rs:21`; `explorer/src/theme.rs:97`; `x11-desktop/src/lib.rs:217`; `wm-x11/src/lib.rs:290`; `desktop/src/icon_raster.rs:30`; `shell/src/projection.rs:91` | partial: rgb done via UiColor::from_rgb. send_state: logic already in `x11-desktop::shared::send_state`, remaining per-type wrappers are adapters (no net line win). runtime_image: `Rgb8Raster` is a type alias of `Rgba8Raster`, so identical converters; blocked on new edge `flamewm-integrations-linux -> flamewm-ui-document` (then `impl From<Rgba8Raster> for RuntimeImage` in integrations-linux) | S | open (needs dep edge) |
| read-U4 | quality | `drain` and `nonblocking_pair` test helpers duplicated | `read.md:97` | test maintenance | S | open |
| read-SIZE | quality | 49 production files over 600 lines; 6 over 1000 | `read.md (line unpinned)-271`; `desktop/src/widgets/mod.rs` 1307 | split by responsibility | L | open |
| read-GUARD | quality | Five pending `TODO(J..-PENDING)` guard warnings, 24-62 days old | `tools/architecture_guard.py:440,518,522,851,859` | governance debt | S | open |
| wl-lock-multi | feature | Session lock per output on Wayland: `lock_cover.rs` tracks coverage (pure, unit-tested; a dead locker uncovers its output). Open: tty path renders one output, so second-output lock surfaces, outputs added while locked and any multi-output runtime run are unproven | `wm-wayland/src/lock_cover.rs`; `wm-wayland/src/session_lock.rs`; `WAYLAND_PLAN.md` item 3 | multi-output lock | M | partial |
| wl-portal | feature | xdg-desktop-portal Screenshot and ScreenCast for Wayland. Landed: zwlr-screencopy full-output shm capture (`bf3af84a`) and Screenshot portal backend `flamewm-screenshot --portal` (`0eaf3888`). Open: ScreenCast; region/window capture on Wayland; Wayland branch of the portal and real xdg-desktop-portal routing unverified | `wm-wayland/src/screencopy.rs`; `screenshot/src/wayland.rs`; `WAYLAND_PLAN.md` item 2 | portal integration | L | partial |
| wl-dmabuf-resume | reliability | Initial `zwp_linux_dmabuf_v1` offer landed (`294250da`); drm resume does not re-run `dmabuf::init`, so the global is not re-offered after resume | `WAYLAND_PLAN.md` item 8 (resume note) | no zwp_linux_dmabuf_v1 after resume (hardware path unproven) | S | open |
| break-b1 | reliability | No session bus: shell exited, no panel; shell retries bus attach and control wait is skipped without a bus | `docs/backlog/break.md:37,39` (B1, B1b) | no panel without bus | M | done d7d1c91e |
| break-s6 | reliability | Malformed or non-UTF-8 settings aborted the WM; now logs and keeps defaults | `docs/backlog/break.md:35`; `crates/flamewm-platform/src/host.rs:89` | WM abort on bad settings | S | done 501b59cd |
| break-b2 | reliability | WM start logs `XSettings X11 request failed: failed to read whole buffer` | `docs/backlog/break.md:27,42-46`: 14 of 30 soak runs; WM stays up, claim non-fatal (`crates/flamewm-wm/src/main.rs:165`) | XSettings not claimed in ~47% of runs; root cause open | M | done (harness Xvfb `-noreset`; root cause in `docs/backlog/break.md`) |
| break-b3 | quality | WM RSS steps 8.2 MB over first 150 churn cycles, then plateaus; no per-cycle leak at this size | `docs/backlog/break.md:28` | soak 1000+ cycles needed | S | open |
| break-b4 | quality | Settings save-error path unexercised (read-only config dir) | `docs/backlog/break.md:29`; `crates/flamewm-platform/src/services/settings_persist.rs:56` | save failure not verified | S | open |
| break-b5 | quality | Label flicker (map count during hover) not measured | `docs/backlog/break.md:30` | no map-count probe | S | open |
| break-b6 | quality | Dock/panel reposition after resize checked by screenshot only | `docs/backlog/break.md:31` | no geometry assertion | S | open |
| wsh-dock-reserve | reliability | Dock strut-covered edge reserved once on Wayland (double reserve) | `docs/backlog/wayland-shared.md` row 5, Step E | Wayland client 58 px too small | S | done 79aa37c7 |
| wsh-grab | quality | Grab state machine (Move/Resize) differs X11 vs Wayland | `docs/backlog/wayland-shared.md:18` row 8 | largest remaining shared-logic item | L | open |
| wsh-strut | quality | Dock strut on work area differs (X11 raw struts vs Wayland unmanaged struts and layer zone) | `docs/backlog/wayland-shared.md:14` row 4 | different inputs, not shared | M | open |
| wsh-lock-x11 | quality | Lock, focus/urgency, media on X11 unmeasured | `docs/backlog/wayland-shared.md:22` row 12 | X11 side unknown | M | open |
| wsh-launcher-select | quality | Launcher selected row: default settings match X11 (20,46,59 both); custom accent and non-black backdrop unmeasured | `WAYLAND_PLAN.md` item 6 (measured table) | darker-row claim unconfirmed beyond defaults | S | open |
