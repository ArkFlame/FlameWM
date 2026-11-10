# Backlog index

Single ranked index of open candidates and recent landings. Detail lives in `docs/backlog/*.md` and `docs/competitors/**`. Snapshot: origin/main `6eef582c`, 2026-11-09. Done rows cite the landing sha. Unknown or unverified status stays `open`. Static evidence only unless a row says otherwise. Rank = order within the table; open rows first.

Ids: `read-*` = docs/backlog/read.md, `meas-*` = measure.md, `wrss-*` = wayland-rss.md, `look-*` = look.md, `dock-*` = dock-translucency.md, `cmp-*` = compare.md, `gap-*` = docs/competitors/vela/GAP_PLAN.md.

| id | area | title | evidence | impact | size | status |
| --- | --- | --- | --- | --- | --- | --- |
| meas-blur | perf | Full-frame CPU blur dominates the frame | `docs/backlog/measure.md:153,170`: blurs 77-83 ms of 83-93 ms batch | up to ~80 ms/frame (estimate) | L | open |
| meas-settings | perf | Settings app startup outlier | `measure.md:148`: 1774 / 1844 ms mapped / painted | up to ~1.5 s per open (estimate) | M | open |
| meas-terminal | perf | Terminal paints ~340 ms after map | `measure.md:145`: 276 / 618 ms | up to ~340 ms per open (estimate) | M | open |
| meas-heap | perf | Idle RSS: 40 MB heap/anon unattributed | `wayland-rss.md:350`; no heap profiler run | unknown until profiled | M | open |
| wrss-scratch | perf | Drop scratch texture after backdrop capture | `wayland-rss.md:368`: -3.0 to -3.5 MB PSS, measured | below 5 MB bar | S | open |
| wrss-gpu | perf | Re-measure idle RSS on a GPU host | `wayland-rss.md:377`: all numbers from llvmpipe | unknown | S | open |
| meas-build | quality | `syn` and `x11rb-protocol` single-unit build cost | `measure.md:159,175`: 50-110 s units | build time, gain unmeasured | M | open |
| read-E1 | reliability | Icon-theme rollback result dropped (data-loss path) | `read.md:221`; pattern gone in `themes/import.rs` | would lose user icon theme | S | done e96156bc |
| read-E2 | reliability | Removable-mount save and auto-mount failures silent | `read.md:222`; no `let _ =` hits in `removable_mounts.rs` | silent mount failure | S | done e96156bc |
| read-E3 | reliability | Desktop settings sync `on_ready` error dropped | `read.md:223`; `desktop/src/main.rs:567` now binds `ready` | stale settings UI | S | done e96156bc |
| read-P1 | reliability | `unreachable!()` on D-Bus string-array input | `read.md:201`; `control-dbus/src/lib.rs:482` now `from_item` with `?` | desktop/shell panic | S | done e96156bc |
| read-E4 | reliability | Icon submit error dropped when queue full | `desktop/src/projection.rs:418` `let _ = self.service.submit(` | missing icons, no log | S | open |
| read-E5 | reliability | Empty `Err(_) => {}` arms in icon loader | `shell/src/icon_loader.rs:269,316,335` | unknown until read | S | open |
| read-E6 | reliability | Lock session settings `connect().ok()` falls back silently | `lock/src/session.rs:36,79` | silent default settings | S | open |
| read-P2 | reliability | `expect` on client compositor data | `wm-wayland/src/state.rs:1132` | compositor panic, hard to reach | S | open |
| read-P3 | reliability | `expect` in XWayland callback after teardown | `wm-wayland/src/xwayland.rs:690` (per read.md) | compositor panic, timing unverified | M | open |
| dock-wl | visual | Dock translucency on Wayland not runtime-verified | `dock-translucency.md:32,63`; `docs/DOCK_BACKDROP.md:109` | visual regression risk | M | open |
| dock-x11 | visual | Dock pill alpha follows opacity; backdrop reinstated after hide (X11) | `dock-translucency.md:31,51-56` 5-run table | dark pill over bright window (fixed on X11) | M | done 7aa3156e, 93448de1 |
| dock-wire | reliability | Dock surface percent keys decoded as Integer | `dock-translucency.md:38`; test `control-wire/src/tests.rs:991` | wrong dock opacity | S | done 32f28041 |
| look-muted | visual | `--muted` still differs in 6 stylesheets | `look.md:107-110`: `assistant.css`, `osd.css`, `archiver.css`, `desktop.css`, `task-manager.css`, `shell/flamewm.css` | colour drift | S | open (base landed, see look-muted-done) |
| look-muted-done | visual | One canonical `--muted` in `ui/base/base.css` | `look.md:77`; `ui/base/base.css:3` | colour consistency | S | done bc8098cd |
| look-radius | visual | Two radius scales in `ui/base/tokens.css` | `look.md:103`; radius pass dropped | token drift | M | open |
| look-radius-large | visual | 14-28 px radii on cards outside scale | `look.md:97-101` (design decisions, not defects) | decision needed | S | open |
| look-harness | quality | Archiver blank, assistant black, task-manager never settles in harness | `look.md:113-115` | blocks visual verification | M | open |
| gap-accent | visual | Shared accent token: route active colours through one accessor | `docs/competitors/vela/GAP_PLAN.md` P1; `flamewm-wm-wayland/src/snap_preview.rs:58` | consistency | S | open |
| cmp-quick | feature | Quick settings in one surface (Wi-Fi, BT, power, brightness) | `cmp-2`; `shell/src/quick_controls/host/run.rs` exists | partial | M | open |
| cmp-power | feature | Power profile switch (UPower PowerProfiles) | `integrations-linux/src/power_profiles.rs`; facade `bd8c55ba`, plumbing `4021f35f` | UI toggle/tile not verified | M | done bd8c55ba, 4021f35f |
| cmp-dnd | feature | Do Not Disturb toggle | `integrations-linux/src/notifications.rs:178` `set_dnd`; `shell/src/main.rs:280` | stale GAP_PLAN:77 claim | S | done (pre-existing, no sha) |
| meas-tm | reliability | Task-manager startup probe never paints | `measure.md:150` (3/3 `painted` None) | fixed by probe change | S | done c759a2a8 (not re-measured) |
| cmp-wm-session-lock | quality | README says "No session lock"; code has it | `README.md:89`; `wm-wayland/src/session_lock.rs` | doc drift | S | open |
| cmp-bt | feature | Bluetooth pairing and in-shell prompts | `cmp-3`; Vela `bluetooth/` 25 files | absent | L | open |
| cmp-polkit | feature | Polkit authentication agent | `cmp-4`; 0 hits | absent, security-sensitive | L | open |
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
| read-D5 | quality | Unused constants in glass, palette, recipes | `read.md:252` | dead code | S | open |
| read-D6 | quality | Dead `set_ring_mask` (dock, other owner) | `render-x11/src/surface_controller/dock.rs:153` | dead code | S | open |
| read-D7 | quality | Unused `titlebar_rect`, `SESSION_LOCK_INTERFACE` | `skin/src/recipes/window_chrome.rs:82`; `wm-wayland/src/lock_global.rs:13` | dead code | S | open |
| read-U1 | quality | Settings accessor duplicated in 5 places | `read.md:279` | maintenance | M | open |
| read-U2 | quality | `node_backdrop_clip` and `scale_rect` duplicated in render-x11 | `read.md:280-281` | maintenance | S | open |
| read-U4 | quality | `drain` and `nonblocking_pair` test helpers duplicated | `read.md:282` | test maintenance | S | open |
| read-SIZE | quality | 49 production files over 600 lines; 6 over 1000 | `read.md:258-271`; `desktop/src/widgets/mod.rs` 1307 | split by responsibility | L | open |
| read-GUARD | quality | Five pending `TODO(J..-PENDING)` guard warnings, 24-62 days old | `tools/architecture_guard.py:440,518,522,851,859` | governance debt | S | open |
| wl-portal | feature | xdg-desktop-portal Screenshot and ScreenCast for Wayland (zwlr-screencopy covers full-output shm capture only; no portal, no dmabuf, no region/window capture, no video stream) | `wm-wayland/src/screencopy.rs`; `screenshot/src/wayland.rs` | portal integration | L | open |
