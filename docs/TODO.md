# FlameWM Rust WebRender — Complete Product TODO

This is the canonical implementation backlog after 0.0.2. A checked item means active production source exists in this branch; it does not mean every runtime corner is verified in the artifact assembly environment.

## Phase 0 — independent branch foundation

- [x] Remove `/engine` and all active IceWM/IceWM-Rust runtime dependencies.
- [x] Keep historical projects under `.vendor/` only.
- [x] Enforce no active `.vendor` path/source dependency.
- [x] Correct workspace version/rust-version metadata.
- [x] Rust 1.85 / edition 2024 workspace contract.
- [x] Independent `flamewm-wm` crate using `x11rb`.
- [x] Flame-owned product/platform/core crate workspace.
- [x] FlameWM Render 0.0.6 source promoted into active renderer crates.
- [x] V8 0.0.6 presentation/assets promoted as active visual baseline.
- [x] `doctor`, `check`, release `xephyr`, `verify-visual`, memory helper.
- [x] SourcePort post-mortem preserved and converted into `docs/PORTING_METHOD.md`.
- [ ] Target machine: `./scripts/check` compiler/test/release gate PASS.
- [ ] Target machine: `./scripts/xephyr` interactive PASS.
- [ ] Target machine: `./verify-visual` differential metrics recorded.

## Phase 1 — WebRender renderer

### Implemented

- [x] Build-time constrained HTML parser.
- [x] Build-time constrained CSS parser/cascade.
- [x] RWRB/2 encoded runtime document.
- [x] Runtime visibility/text/color/geometry mutation.
- [x] Runtime flex-direction/background/border mutation.
- [x] Flex/block/absolute layout required by V8 baseline.
- [x] Typed `data-action` hit testing.
- [x] Xlib/Xft native backend.
- [x] IBM Plex Sans preferred through Xft/Fontconfig.
- [x] Cached X11 colors/fonts/images.
- [x] Build-time raster asset embedding.
- [x] Magenta-key transparency -> cached 1-bit X11 masks.
- [x] Rounded fill and rounded stroke rendering.
- [x] Native pointer cursors.
- [x] Press/Motion/Release action phases.
- [x] Contiguous MotionNotify coalescing without crossing non-motion/release events.
- [x] GraphicsExpose suppression for cached image copies.
- [x] Release-optimized interactive path.

### Remaining renderer primitives

- [ ] Explicit dirty rectangles/damage union; stop full-surface redraw when a bounded control changes.
- [ ] Clip-stack support for overflow clipping.
- [ ] Native scroll container state.
- [ ] Keyboard focus traversal.
- [ ] UTF-8 text input path.
- [ ] Text selection/input caret where required.
- [ ] Generic pointer-capture abstraction reusable by shell/desktop/task reorder.
- [ ] Double-click semantic event.
- [ ] Wheel event semantic path.
- [ ] Runtime element-class/state mutation only where exact V8 semantics require it; avoid generic browser DOM recreation.
- [ ] Dynamic text measurement API for popup/search/result sizing.
- [ ] Optional ARGB/RGBA image format after profiling; retain current keyed-mask path until a real alpha requirement justifies it.
- [ ] Per-output scale-aware image/font cache keys.

## Phase 2 — independent FlameWM X11 WM

### Implemented foundation

- [x] Root SubstructureRedirect ownership.
- [x] Existing-client scan.
- [x] New MapRequest management.
- [x] Frame creation/reparenting.
- [x] X11 SaveSet lifecycle.
- [x] Client/frame registry.
- [x] Basic frame drawing/title/buttons.
- [x] Focus/activation.
- [x] ICCCM close protocol fallback.
- [x] WM_STATE publication.
- [x] `_NET_SUPPORTED` / `_NET_SUPPORTING_WM_CHECK` / `_NET_WM_NAME`.
- [x] `_NET_CLIENT_LIST` / `_NET_CLIENT_LIST_STACKING` foundation.
- [x] `_NET_ACTIVE_WINDOW`.
- [x] `_NET_FRAME_EXTENTS`.
- [x] `_NET_WM_STATE` maximize/fullscreen/hidden foundation.
- [x] `_NET_WM_DESKTOP`, current desktop and desktop count.
- [x] desktop geometry/viewport/workarea/names publication.
- [x] interactive titlebar move.
- [x] interactive edge resize foundation.
- [x] minimize/restore foundation.
- [x] maximize/restore.
- [x] fullscreen.
- [x] half/quarter/top-maximize snap geometry.
- [x] negative-origin-safe pure geometry tests.

### Required WM completion

- [ ] Full ICCCM WM_NORMAL_HINTS: min/max/base/increments/aspect/gravity.
- [ ] WM_TRANSIENT_FOR ownership/stacking/placement.
- [ ] WM_HINTS input/model/icon urgency semantics.
- [ ] WM_TAKE_FOCUS and complete focus model.
- [ ] Focus stealing prevention/user-time policy.
- [ ] ConfigureRequest semantics for framed/unframed windows including stack/sibling.
- [ ] Synthetic ConfigureNotify exactness after every WM geometry mutation.
- [ ] Colormap windows/focus support where needed.
- [ ] Client leader/window groups.
- [ ] Modal/sticky/above/below/attention/shaded states as product requires.
- [ ] `_NET_WM_ALLOWED_ACTIONS`.
- [ ] `_NET_MOVERESIZE_WINDOW` / `_NET_WM_MOVERESIZE`.
- [ ] `_NET_REQUEST_FRAME_EXTENTS`.
- [ ] `_NET_WM_FULLSCREEN_MONITORS`.
- [ ] show-desktop semantics.
- [ ] exact stacking order authority instead of ID-sorted placeholder publication.
- [ ] raise/lower/restack policy.
- [ ] titlebar double-click maximize.
- [ ] drag-away restore with pointer-relative anchoring.
- [ ] live snap preview driven by WM drag state, not only showcase simulation.
- [ ] edge-dwell dragged-window workspace traversal.
- [ ] indexed workspace insertion/removal + deterministic window migration.
- [ ] keyboard workspace directional topology.
- [ ] RandR output topology and hotplug.
- [ ] per-output work areas and struts.
- [ ] correct behavior on negative/non-zero output origins.
- [ ] frame re-scaling/re-decoration when crossing outputs.
- [ ] XKB-aware modifier/key grabs and reload-safe shortcut ownership.
- [ ] session shutdown/unmanage restoration proof.
- [ ] crash/restart recovery for already-reparented clients.
- [ ] differential ICCCM/EWMH fixture suite against mature reference behavior where compatibility matters.

## Phase 3 — V8 shell parity

### Implemented in native WebRender controller

- [x] V8 wallpaper/watermark visual baseline.
- [x] V8 desktop icon geometry baseline.
- [x] Start menu open/close.
- [x] Start category/submenu presentation.
- [x] Start application launch actions for core demo paths.
- [x] mutually-exclusive Start/context/tray popovers.
- [x] Settings window and seven V8 pages.
- [x] Settings minimize/maximize/restore/close simulation.
- [x] Terminal showcase window.
- [x] Native movable showcase windows.
- [x] snap preview + half/quarter/maximize showcase behavior.
- [x] drag-away floating restore in showcase.
- [x] desktop rectangle selection.
- [x] draggable desktop entries + V8 grid snapping.
- [x] New Folder showcase entry/context action.
- [x] semantic live accent variables.
- [x] media/audio/Wi-Fi/calendar popovers.
- [x] media play/pause/prev/next state.
- [x] volume/mute state.
- [x] selectable Wi-Fi demo state.
- [x] About Donate/Source/Website actions.
- [x] exact 44px bottom taskbar V8 baseline.
- [x] corrected V8/Breeze asset mappings from 0.0.6.

### Required shell integration

- [ ] Replace showcase window registry with real WM window snapshots for task/chrome surfaces.
- [ ] Real task entries from managed windows.
- [ ] one task entry per open window; grouping disabled by Flame default.
- [ ] persistent pin identity separate from live window identity.
- [ ] inactive pinned launchers.
- [ ] V8 insertion-based task reorder before/between/after.
- [ ] click-vs-drag threshold.
- [ ] orientation-aware task reorder axis.
- [ ] persist pin/order model.
- [ ] task context menu exact V8 state matrix.
- [ ] Start application model from `.desktop` files.
- [ ] Start search text input/filtering.
- [ ] categories sourced from Freedesktop metadata.
- [ ] exact application icons through icon-theme lookup/fallback policy.
- [ ] power/session actions wired to session service.
- [ ] real taskbar workspace pager from WM snapshot.
- [ ] two-row topology and up/down/left/right navigation.
- [ ] selected workspace context add/remove.
- [ ] taskbar Bottom/Top/Left/Right renderer and strut integration.
- [ ] direct blank-surface drag docking with preview.
- [ ] semantic Compact/Default/Large taskbar metrics.
- [ ] taskbar per-output instances.
- [ ] active-output Start routing.
- [ ] clock/date from real local time.
- [ ] calendar generated from real current month/date.
- [ ] battery when present.
- [ ] system tray/XEmbed host.

## Phase 4 — desktop filesystem

- [ ] XDG Desktop directory discovery.
- [ ] inotify event-driven filesystem watcher.
- [ ] stable desktop entry identity.
- [ ] icon-theme resolution for files/folders/apps.
- [ ] persisted V8 grid positions.
- [ ] conflict-free deterministic placement.
- [ ] selection rectangle from real desktop entries.
- [ ] Ctrl/Shift multi-selection semantics.
- [ ] group drag preserving relative positions.
- [ ] rename/create/delete/move reconciliation.
- [ ] double-click open.
- [ ] context file/folder actions.
- [ ] Freedesktop Trash implementation.
- [ ] drag-to-Trash.
- [ ] Trash empty/full icon state.
- [ ] sticky-note persistence if retained by current product contract.

## Phase 5 — settings

- [ ] One effective `SettingsSnapshot` authority.
- [ ] Atomic persistence under XDG config.
- [ ] schema version and migrations.
- [ ] immediate safe apply for appearance/accent/wallpaper/taskbar/font settings.
- [ ] rollback on native apply/persistence failure.
- [ ] semantic Settings icon resolver with Breeze + packaged fallback.
- [ ] dark/light appearance variants.
- [ ] custom accent picker.
- [ ] wallpaper picker/recent list/fit modes.
- [ ] overlay selection opacity.
- [ ] snap-preview opacity.
- [ ] taskbar edge/size persistence.
- [ ] font family/size/global bold persistence.
- [ ] curated hotkey registry.
- [ ] hotkey capture.
- [ ] Escape -> Not assigned.
- [ ] per-row restore default.
- [ ] conflict detection before native key-grab commit.
- [ ] About version/build/source/site/donation actions.

## Phase 6 — displays and scale

- [ ] RandR output model with connector + EDID-stable identity.
- [ ] selected-output-first Settings UX.
- [ ] mode enumeration.
- [ ] resolution apply transaction.
- [ ] Keep/Revert countdown owned outside the Settings process.
- [ ] crash/timeout rollback.
- [ ] output enable/disable/position/rotation when approved.
- [ ] per-output Flame shell scale 100/125/150/175/200.
- [ ] scale-keyed font/image/metric caches.
- [ ] live frame/shell migration across different-scale outputs.
- [ ] hotplug cleanup/reassignment for panel/popovers/Start.

## Phase 7 — system integrations

### D-Bus

- [ ] one bounded event-loop reactor.
- [ ] owner/reconnect handling.
- [ ] typed service snapshots.

### NetworkManager

- [ ] real network state.
- [ ] device/AP subscription.
- [ ] Wi-Fi list on demand.
- [ ] connect/disconnect actions.
- [ ] secret-agent-compatible credential path.
- [ ] no periodic `nmcli` polling.

### Audio

- [ ] asynchronous PulseAudio-compatible API.
- [ ] default sink/state subscription.
- [ ] volume/mute controls.
- [ ] no periodic `wpctl` polling.

### MPRIS

- [ ] player discovery/owner tracking.
- [ ] active-player policy.
- [ ] metadata/title/artist.
- [ ] capability-aware prev/play/next.
- [ ] no periodic `playerctl` polling.

## Phase 8 — session/product integration

- [ ] FlameWM session desktop file.
- [ ] startup/shutdown lifecycle.
- [ ] external notification-daemon policy.
- [ ] Polkit agent policy.
- [ ] screen-lock command/integration.
- [ ] power/logout/reboot/shutdown implementation.
- [ ] XDG environment setup.
- [ ] toolkit appearance variables/defaults.
- [ ] recovery if optional integrations are absent.

## Phase 9 — performance and release

- [ ] benchmark PSS for every owned resident process.
- [ ] enforce <= 40 MiB target or record exact justified exception.
- [ ] idle CPU/wakeup measurement.
- [ ] interaction latency measurement for drag/selection/task reorder.
- [ ] prove no stale motion replay after release.
- [ ] startup latency measurement.
- [ ] file-descriptor/thread counts.
- [ ] no recurring subprocess polling.
- [ ] ASan/Valgrind equivalents where applicable to FFI renderer boundary.
- [ ] complete license/provenance audit.
- [ ] release archive reproducibility/manifest verification.
- [ ] target-machine `./scripts/check` green.
- [ ] `./scripts/xephyr` real interactive smoke green.
- [ ] `./verify-visual` reviewed against approved reference.
