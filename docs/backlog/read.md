# READ discovery backlog

> Rows: `read-*` in `docs/BACKLOG.md` (consolidated status there). This file keeps the per-row evidence.
> Status re-verified against `origin/main` at `92a40bf0` by grep/read of current source, not from memory. Cargo not run. Static evidence only.
> Original discovery base: `c5955ae3`. Method: `#[cfg(test)]`-dropping scan of `crates/*/src/**`, `rg`, `git log -S`.

## 1. Panic-class sites

| # | Location (verified) | Status | Evidence |
|---|---|---|---|
| P1 | `control-dbus/src/lib.rs:473` `from_item` | done | `e96156bc`; now returns `Result`, no `unreachable!` in production |
| P2 | `wm-wayland/src/state.rs:1145` | done | `94be79e1`; no `expect` left in `state.rs` |
| P3 | `wm-wayland/src/xwayland.rs:687-690` | open | `xwm_state` still `expect`s; Smithay trait returns `&mut X11Wm`, no `Option`. Runtime timing unverified |
| P4 | `integrations-linux/src/mpris.rs:568,575` | won't-fix | `575` `unwrap_or_else(unreachable!)` is guarded by the early return at `568`; `let-else` would not shrink code |
| P5 | `control-dbus/src/mutation_queue.rs:183-197` | done | `d8dfa39e`; `drain_queue_inline` sits under `#[cfg(test)]` |
| P6 | `integrations-linux/src/icon_service.rs:263,622` | open | `expect` on thread spawn remains. Shell `icon_loader.rs` expects are gone |

Not findings (kept from discovery): reactor tests at `reactor/src/lib.rs:482-546`, `settings_sync.rs` test-only `panic!`, `network_manager/mod.rs:445`, `control_actions.rs:214`, `shell/src/main.rs:687`.
Other workers' areas, not reviewed: `shell/src/launch_feedback.rs:231`, `shell/src/app/dock.rs`.

## 2. Swallowed errors

| # | Location (verified) | Status | Evidence |
|---|---|---|---|
| E1 | `integrations-linux/src/themes/import.rs:220-231` | done | `e96156bc`; `restore_failed_install` names the backup path on rollback failure |
| E2 | `integrations-linux/src/removable_mounts.rs:340-352` | done | `e96156bc`; save and mount failures go to `eprintln!` |
| E3 | `desktop/src/main.rs:567,639` | done | `e96156bc`; `ready` bound, `settings_sync_failure_line` logs it |
| E4 | `desktop/src/projection.rs:418-425` | done | `093a850c`; logs the first drop, then counts silently |
| E5 | `shell/src/icon_loader.rs:374` | done | `093a850c`; the empty `Err(_) => {}` arms are gone, only `=> break` remains |
| E6 | `lock/src/session.rs:36-40` | done | `093a850c`; connect failure logged before defaults |

Excluded by design: `shell/src/quick_controls/supervisor.rs:310,321`; dock `let _ =` at `shell/src/app/dock.rs` (other owner).

## 3. Settings keys

| # | Location (verified) | Status | Evidence |
|---|---|---|---|
| K1 | `shell/src/launch_feedback.rs` | done | `d8dfa39e`; no `launchFeedbackAnimation` literal left in `crates/` |
| K2 | `platform/src/settings/stable.rs:208-209` | won't-fix | intentional legacy alias `wallpaperShare` to `shareWallpaper` |
| K3 | `platform/src/settings/stable.rs:129`, `settings/src/tables.rs:13` | won't-fix | intentional UI alias |

## 4. Dead code

| # | Location (verified) | Status | Evidence |
|---|---|---|---|
| D1 | `tray/src/watcher.rs` `registered_items` | done | `d8dfa39e`; zero hits |
| D2 | `ui-document/src/template.rs:94` `runtime_document` | open | zero callers of `.runtime_document(` (only the definition) |
| D3 | `integrations-linux/src/themes/mod.rs:132` `icon_chain` | open | called only from `tests/theme_catalog.rs:134` |
| D4 | `platform/src/host.rs:364` `flush_settings` | open | called only from `control-core/tests/platform_control.rs:451`; confirm before gating |
| D5 | `ui-core/src/glass.rs:26-28`; `skin/src/palette.rs:26,30,242`; `skin/src/recipes/{button.rs:12,sticky.rs:13,taskbar.rs:10}` | open | each name has exactly one occurrence (its definition) |
| D6 | `render-x11/src/surface_controller/dock.rs:153` `set_ring_mask` | open | zero callers; dock owner |
| D7 | `skin/src/recipes/window_chrome.rs:82` `titlebar_rect`; `wm-wayland/src/lock_global.rs:13` `SESSION_LOCK_INTERFACE` | open | zero callers |

## 5. File size

- `desktop/src/widgets/mod.rs` 1460 lines (was 1307); `lock/src/run.rs` 1262 (was 1130). Both still over the 600-line limit in `docs/DEBT_REGISTER.md:15`.
- Re-measured only these two files. The 49-file count was not re-run on the discovery basis (non-blank, non-comment, test-files excluded), so it is unverified. Open, size L.

## 6. Duplicated helpers

| # | Location (verified) | Status | Evidence |
|---|---|---|---|
| U1 | `flamewm-api/src/settings.rs` (`boolean_value`/`percent_value`/`integer_value`/`text_value`, `setting_*`/`setting_text` defaults); `settings/src/app_impl_2.rs` percent/integer/bool readers delegate; wm-x11, shell settings_projection, wm main re-use api | done | deduped; `blue_light` percent reader stays local (accepts Integer 0..=100, distinct semantics) |
| U2 | `render-core/src/lower.rs:204` | done | `871b4d33`; render-x11 copy re-exports core |
| U3 | `render-core/src/lower.rs:126` | done | `871b4d33` |
| U4 | `finder/src/lib.rs:270,284`; `assistant/src/lib.rs:981,995` | open | still duplicated |
| U5 | `x11-desktop/src/shared.rs:156`; `x11-desktop/src/lib.rs:217`; `wm-x11/src/lib.rs:290` | open | per-type adapters over `shared::send_state`; no net line win (see `BACKLOG.md` dup-color-rgb) |
| U6 | `reactor/src/lib.rs:337`; `shell-core/src/clock.rs:40` | open | both present; needs dep-edge decision |
| U7 | `flamewm-ui-document/src/template.rs` `RuntimeImage::new` / `from_rgba` (single owner); desktop, shell wrappers and lock use it | done | deduped; desktop/shell keep one-line adapters for their raster types |
| U8 | `finder/src/instance.rs:63`; `assistant/src/instance.rs:69`; `integrations-linux/src/device_watch.rs:115`; `explorer/src/refresh.rs:114` | open | still duplicated |
| U9 | `ui-x11/src/surface_runtime.rs:25`; `render-x11/src/config.rs:67` | won't-fix | two different `SurfaceRole` types and two target types; not a duplicate |

## 7. Guard TODOs

| Location (verified) | Status |
|---|---|
| `tools/architecture_guard.py:440,518,522,851,859` | open; all five still present |

Zero production `TODO`/`FIXME`/`HACK`/`XXX` markers. The two `XXX` hits in `assistant/src/voice_setup.rs` are `mktemp` templates.

## Not searched

- `.vendor/**`, `docs/competitors/**`, dock and launch-feedback logic (other owners).
- Cargo build, test, clippy, and runtime. Nothing here has runtime evidence.
- Function-length metric (158 cited from `docs/QUALITY_SCORECARD.md:21`, not re-measured).
- Near-duplicate or renamed-token clones.
