# READ discovery backlog

> Rows: `read-*` (E, P, K, D, U, SIZE, GUARD ids) in `docs/BACKLOG.md`.

Base: `origin/main` at `c5955ae3`. Read-only discovery. No product code changed. Cargo not run.
Size: S = under 20 lines, M = 20 to 100 lines, L = over 100 lines or cross-crate.
Method: a Python scan over `crates/*/src/**` that drops `#[cfg(test)]` blocks, plus `rg`/`grep` and `git blame`.
Caveat: the scan drops only `#[cfg(test)]` blocks, not `#[cfg(all(test, ...))]`, so some counts include test code.

## 1. Panic-class sites in long-running processes (external input)

Counts: 76 `unwrap`/`expect`/`panic!`/`unreachable!` hits across shell, desktop, wm-x11, wm-wayland, reactor, control-dbus, integrations-linux, lock, terminal and explorer. Most are infallible writes to `String` or internal invariants.

| # | file:line | Why it matters | Proposed minimal fix | Size | Risk |
|---|---|---|---|---|---|
| P1 | `crates/flamewm-control-dbus/src/lib.rs:482` | `from_item` maps a D-Bus `as` array with `unreachable!()` on any non-string element. The input comes from any D-Bus client. dbus-rs should enforce element types, so this is probably unreachable, but a panic here kills the shell or desktop. | Return `Err(FlameError::invalid("D-Bus string array element must be string"))`, in the same style as the Dict arm at line 490. | S | Low likelihood, high impact |
| P2 | `crates/flamewm-wm-wayland/src/state.rs:1129-1133` | `client_compositor_state` panics with `expect("every client carries compositor data")` when a client has neither `ClientState` nor `XWaylandClientData`. `main.rs:656` inserts `ClientState` for every normal client, so this is hard to reach. | Return a default `CompositorClientState` or a `Result`, or log and skip. | S | Low |
| P3 | `crates/flamewm-wm-wayland/src/xwayland.rs:690` | `xwm_state` panics if an XWayland callback fires after the X11 WM is torn down. | Make `xwm_state` return an `Option`, or guard the callback. | M | Low, unverified timing |
| P4 | `crates/flamewm-integrations-linux/src/mpris.rs:575` | `unwrap_or_else(\|\| unreachable!())` on `pending.player`. Line 568 returns early when `player` is `None`, so this is provably unreachable. Players come from the session bus. | Use `let Some(mut info) = pending.player.take() else { return Ok(()) };`. | S | Negligible; cosmetic |
| P5 | `crates/flamewm-control-dbus/src/mutation_queue.rs:196` | `drain_queue_inline` is `pub` in a production file. It calls `push_result(..).expect("write mutation wake byte")`. It is a test oracle, so the impact is limited. | Move it under `#[cfg(test)]`, or return the error. | S | Low |
| P6 | `crates/flamewm-integrations-linux/src/icon_service.rs:261,617` and `icon_loader.rs:173,176,223` in shell | `expect` on thread spawn and `UnixStream::pair()`. These fail only under resource exhaustion, and they run at startup or on the icon path. | Propagate as `FlameResult` and log, or keep as a startup-only abort with a clear message. | M | Low |

Checked and not findings:
- `flamewm-reactor/src/lib.rs:482-546`: inside `#[cfg(all(test, unix))] mod tests`.
- `flamewm-desktop/src/settings_sync.rs:4,8` `panic!`: `source_between` is used only by a test in `workspace_sync.rs:104`.
- `flamewm-integrations-linux/src/network_manager/mod.rs:445`: `from_dict` on a constant empty iterator cannot fail.
- `flamewm-shell/src/control_actions.rs:214`: the `None` dispatcher path is logged and counted, so it is not silent.
- `flamewm-shell/src/main.rs:687`: `MutationDispatcher::start(..).ok()` is surfaced by the path above, so I downgraded it.

Not individually reviewed, all in other workers' areas: `flamewm-shell/src/launch_feedback.rs:231` (`expect("checked above")`), and `flamewm-shell/src/app/dock.rs` (dock).

## 2. Silently swallowed errors that hide user-visible failure

| # | file:line | Why it matters | Proposed minimal fix | Size | Risk |
|---|---|---|---|---|---|
| E1 | `crates/flamewm-integrations-linux/src/themes/import.rs:221-223` | Icon-theme overwrite: `fs::rename(dest, &backup)` succeeds, then `rename(staging, dest)` fails. The rollback `let _ = fs::rename(&backup, dest)` discards its own failure and returns the original error. If the rollback fails, the user's existing icon theme is gone, and the error does not mention it. | Match the rollback result. On failure, return a combined error that includes the backup path. | S | Data-loss path. Medium priority. |
| E2 | `crates/flamewm-integrations-linux/src/removable_mounts.rs:339` and `:342-343` | `let _ = save(&path, &memory.known)` drops the persistence error. `let _ = udisks_action("mount", ..)` drops every auto-mount failure, so a failed mount is invisible to the user. | Log `Err` with `eprintln!`/`tracing::warn!`, matching the shell's `enqueue_action` convention. | S | Low |
| E3 | `crates/flamewm-desktop/src/main.rs:557` | `let _ = client.on_ready(..)`. `ControlClient::on_ready` returns `FlameResult<usize>` (`control-dbus/src/lib.rs:359`). If the bus read fails, desktop settings sync stops, and nothing is logged. | Log the `Err` and drop the watch fd if the call fails. | S | Medium. Stale settings UI. |
| E4 | `crates/flamewm-desktop/src/projection.rs:418` | `let _ = self.service.submit(IconJob{..})`. `submit` returns `Err(IconSubmitError)` when the queue is full (capacity 256). The icon is then missing with no log. | Log the error, or retry on the next projection. | S | Low |
| E5 | `crates/flamewm-shell/src/icon_loader.rs:268, 315, 334` | Empty `Err(_) => {}` arms. I did not read the surrounding code, so I cannot say what is dropped. | Read the arms, then log or propagate. | S | Unknown |
| E6 | `crates/flamewm-lock/src/session.rs:36` and `:79`, `crates/flamewm-lock/src/power.rs:39` | `ControlClient::connect(..).ok()`. The lock screen fails closed by design, per the comment at `power.rs:37`. A session-settings read failure falls back to defaults with no log. | Log once at connect failure. | S | Low |

Excluded by design: `flamewm-shell/src/quick_controls/supervisor.rs:310,321`, documented best-effort shutdown.

Excluded: dock `let _ = runtime.set_dock_frame(..)` and `set_surface_backdrop` at `flamewm-shell/src/app/dock.rs:463,513,530,537,541`. The dock is owned by another worker. Report only.

## 3. Settings keys defined but not read, or read but not defined

Method: all camelCase string literals in production code, compared with the schema table in `crates/flamewm-platform/src/settings/stable.rs`. Many keys are read through typed struct fields, so only literal hits are conclusive.

| # | file:line | Finding | Proposed minimal fix | Size | Risk |
|---|---|---|---|---|---|
| K1 | `crates/flamewm-shell/src/launch_feedback.rs:328` | Reads `s.values.get("launchFeedbackAnimation")`. No schema row defines that key in `stable.rs`, so the lookup is always `None` and the flag cannot be enabled through the settings schema. | Add the row to `stable.rs` with its default, or delete the dead branch. | S | Medium. Owned by the launch-feedback worker. Report only. |
| K2 | `crates/flamewm-platform/src/settings/stable.rs:208-209` | `wallpaperShare` is a legacy alias mapped to `shareWallpaper`. Intentional, not a defect. | None. | 0 | None |
| K3 | `crates/flamewm-platform/src/settings/stable.rs:129` and `crates/flamewm-settings/src/tables.rs:13` | `taskbarCustomTint` is defined and used as a field. `taskbarTint` is a UI alias. Consistent, not a defect. | None. | 0 | None |

## 4. Dead code and unused items

Method: `pub fn|struct|enum|const|static|type` names whose identifier occurs once across the whole repo, including tests (9,397 pub items scanned). This misses macro or string references.

| # | file:line | Finding | Proposed minimal fix | Size | Risk |
|---|---|---|---|---|---|
| D1 | `crates/flamewm-tray/src/watcher.rs:114` | `registered_items` has zero references. | Delete. | S | Low |
| D2 | `crates/flamewm-ui-document/src/template.rs:94` | `runtime_document` has zero references. | Delete. | S | Low |
| D3 | `crates/flamewm-integrations-linux/src/themes/mod.rs:132` | `icon_chain` is used only by `tests/theme_catalog.rs:134`. | `#[cfg(test)]`, or delete with the test. | S | Low |
| D4 | `crates/flamewm-platform/src/host.rs:348` | `flush_settings` is used only by `crates/flamewm-control-core/tests/platform_control.rs:451`. | Confirm no caller in production code, then gate or delete. | S | Low. Confirm first. |
| D5 | `crates/flamewm-ui-core/src/glass.rs` (`POPOVER_RADIUS`, `MENU_RADIUS`, `BUTTON_RADIUS`); `crates/flamewm-skin/src/palette.rs` (`DESKTOP_HOVER`, `DESKTOP_SELECTED`, `LARGE_MIN_CONTRAST`); `crates/flamewm-skin/src/recipes/{button,sticky,taskbar}.rs` (`TITLE_BUTTON`, `STICKY`, `TASKBAR`) | Unused constants. | Delete unused constants. | S each | Low |
| D6 | `crates/flamewm-render-x11/src/surface_controller/dock.rs:153` | `set_ring_mask` has zero references. The dock is owned by another worker. Report only. | Report only. | S | Low |
| D7 | `crates/flamewm-wm-wayland/src/lock_global.rs` (`SESSION_LOCK_INTERFACE`), `crates/flamewm-skin/src/recipes/window_chrome.rs` (`titlebar_rect`), `crates/flamewm-ui-document/src/template.rs` | Unused items, as listed above. | Delete. | S | Low |

## 5. File and function size

- Production files: 993 (test-named files excluded; `#[cfg(test)]` dropped). Over 600 non-blank, non-comment lines: 49. Over 1000: 6. The count matches the 49 in `docs/ARCHITECTURE.md` §9.
- Largest, all over the 600-line file limit (`docs/DEBT_REGISTER.md:15`):
  - `crates/flamewm-desktop/src/widgets/mod.rs` 1307
  - `crates/flamewm-lock/src/run.rs` 1130
  - `crates/flamewm-integrations-linux/src/icons/mod.rs` 1068
  - `crates/flamewm-assistant/src/lib.rs` 1050
  - `crates/flamewm-render-x11/src/app/mod.rs` 1037
  - `crates/flamewm-wm-x11/src/lib.rs` 1016
  - `crates/flamewm-task-manager/src/lib.rs` 960
  - `crates/flamewm-wm-x11/src/runtime.rs` 955
- Functions over 100 lines: not re-measured. `docs/QUALITY_SCORECARD.md:21` cites 158. Use the scorecard as the reference.
- Crate-level budgets in `tools/architecture_guard.py:163-168` are core 2400, facade 5000, compiler 2400, reactor 1200. I did not check every crate against them.

Fix per file: split by responsibility, as the scorecard prescribes. Size L each. Risk: medium, because the widget and icon modules are wide. Coordinate with the owners of the adjacent work.

## 6. Duplicated helpers

Method: function bodies of 6+ lines, whitespace-stripped and MD5-hashed, shared across different crates. This is exact-match only. A token-based or renamed-identifier clone search was not run, so near-duplicates are missed.

| # | Duplicate | Locations | Proposed minimal fix | Size | Risk |
|---|---|---|---|---|---|
| U1 | Settings accessor, 3 copies plus 2 `boolean_value` copies | `crates/flamewm-api/src/settings.rs:101,109,117` (`setting_bool`, `setting_percent`, `setting_integer`); `crates/flamewm-wm-x11/src/live_settings.rs:95` (`boolean_setting`); `crates/flamewm-shell/src/settings_projection/settings.rs:34,41,48` (`bool_setting`, `integer_setting`, `percent_setting`); `crates/flamewm-api/src/blue_light.rs:92` and `crates/flamewm-wm/src/main.rs:312` (`boolean_value`) | Keep the `flamewm-api` versions, and delete the rest by re-export. | M | Low |
| U2 | `node_backdrop_clip`, 16 lines | `crates/flamewm-render-core/src/lower.rs:204` and `crates/flamewm-render-x11/src/app/surface_backdrop.rs:378` | Move to `flamewm-render-core`, which already owns lowering. | S | Low |
| U3 | `scale_rect`, 8 lines | `crates/flamewm-render-core/src/lower.rs:126` and `crates/flamewm-render-x11/src/app/primitives.rs:110` | Same as U2. | S | Low |
| U4 | `drain` (13 lines) and `nonblocking_pair` (10 lines) | `crates/flamewm-assistant/src/lib.rs:981,994` and `crates/flamewm-finder/src/lib.rs:270,283` | Move to a shared test-util or small core crate. | S | Low |
| U5 | `send_state`, 10 lines | `crates/flamewm-x11-desktop/src/lib.rs:217` and `crates/flamewm-wm-x11/src/lib.rs:290` | Keep in `flamewm-wm-x11`, and re-export. | S | Low |
| U6 | `wall_clock_delay_ms`, 9 lines | `crates/flamewm-shell-core/src/clock.rs:40` and `crates/flamewm-reactor/src/lib.rs:294` | Keep in `flamewm-reactor`, which owns time sources. | S | Low |
| U7 | `runtime_image`, 8 lines | `crates/flamewm-desktop/src/icon_raster.rs:29` and `crates/flamewm-shell/src/projection.rs:90` | Move to a shared crate. | S | Low |
| U8 | `release` (6 lines); `drop` (6 lines) | `crates/flamewm-assistant/src/instance.rs:69` and `crates/flamewm-finder/src/instance.rs:63`; `crates/flamewm-integrations-linux/src/device_watch.rs:115` and `crates/flamewm-explorer/src/refresh.rs:113` | Share the single-instance lock helper. | S | Low |
| U9 | `from`, 10 lines | `crates/flamewm-ui-x11/src/surface_runtime.rs:25` and `crates/flamewm-render-x11/src/config.rs:67` | Keep one owner. | S | Low |

## 7. TODO, FIXME, HACK and guard warnings

- Production Rust: zero `TODO`, `FIXME`, `HACK` or `XXX` markers. The two `XXX` hits (`crates/flamewm-assistant/src/voice_setup.rs:46,56`) are `mktemp` templates.
- `TODO(J..-PENDING)` in `tools/architecture_guard.py`, with `git blame` dates:

| Line | Tag | Introduced | Age on 2026-11-09 | Commit |
|---|---|---|---|---|
| 440 | `TODO(J12-PENDING)`: promote J6-G5/G6 to violations | 2026-09-08 | 62 days | `ecda842ca` |
| 518 | `WARN TODO(J12-PENDING) j6-g5-system-before-packaged` | 2026-10-16 | 24 days | `4eb737762` |
| 522 | `WARN TODO(J12-PENDING) j6-g6-no-new-zindex-store` | not blamed | unknown | not run |
| 851 | `WARN TODO(J07-PENDING) j10-g03-parent-native-surfaces` | 2026-09-13 | 57 days | `a87791d75` |
| 859 | `WARN TODO(J07-PENDING) j10-g05-fitted-start-placement` | 2026-09-13 | 57 days | `a87791d75` |

Proposed fix for all five: either promote each to a hard violation with the migration landed, or convert the warning to a dated entry in `docs/DEBT_REGISTER.md`. Size S. Risk low.

## Ranked top 15

1. **E1** `crates/flamewm-integrations-linux/src/themes/import.rs:221-223`: rollback failure discarded; the user's icon theme can be lost. S. Medium.
2. **E2** `crates/flamewm-integrations-linux/src/removable_mounts.rs:339,342-343`: auto-mount and save failures silent. S. Low.
3. **E3** `crates/flamewm-desktop/src/main.rs:557`: `on_ready` error dropped, so settings sync stops silently. S. Medium.
4. **P1** `crates/flamewm-control-dbus/src/lib.rs:482`: `unreachable!()` on D-Bus input. S. Low likelihood, high impact.
5. **E4** `crates/flamewm-desktop/src/projection.rs:418`: icon submit error dropped; icons go missing silently. S. Low.
6. **P2** `crates/flamewm-wm-wayland/src/state.rs:1129-1133`: `expect` on client data. S. Low.
7. **P3** `crates/flamewm-wm-wayland/src/xwayland.rs:690`: `expect` in XWayland handler. M. Low.
8. **K1** `crates/flamewm-shell/src/launch_feedback.rs:328`: reads an undefined setting key. S. Medium, owned by another worker; report only.
9. **U1** settings accessor duplicated across five locations. M. Low.
10. **E5** `crates/flamewm-shell/src/icon_loader.rs:268,315,334`: empty error arms, not read. S. Unknown.
11. **P4** `crates/flamewm-integrations-linux/src/mpris.rs:575`: provably unreachable `unreachable!()` that should be `let-else`. S. Negligible.
12. **D1, D2, D3** dead items in `tray/watcher.rs:114`, `ui-document/template.rs:94`, `integrations-linux/themes/mod.rs:132`. S. Low.
13. **U2, U3** `render-core/lower.rs` duplicates of `node_backdrop_clip` and `scale_rect` in `render-x11`. S. Low.
14. **Size** `crates/flamewm-desktop/src/widgets/mod.rs` 1307 lines and 5 other files over 1000 lines. L. Medium.
15. **Guard TODOs** `tools/architecture_guard.py:440,518,851,859`: pending warnings 24 to 62 days old. S. Low.

## Did not search

- `.vendor/**` and `docs/competitors/**`: out of scope.
- Cargo build, test or clippy: not run, per instructions. All findings are static, and none has runtime evidence.
- Full panic audit of `flamewm-wm` (legacy path), `flamewm-explorer` and `flamewm-terminal` beyond the regex scan.
- Function-level size: not re-measured. The scorecard figure of 158 functions over 100 lines is cited, not verified.
- Near-duplicate or renamed-token clones: only exact whitespace-normalized bodies were compared.
- Dock, label and launch-feedback logic: not analyzed, as instructed. Issues noted only.
- `Err(_) => {}` context in `icon_loader.rs`, and whether `control_actions` surfaces the `None` dispatcher path to the UI beyond stderr.
- Settings keys read through typed fields: the literal-based check cannot see them. K1 is the only confirmed mismatch.

Risks the user must know: E1 can delete a user's icon theme when a rollback fails, and the fix should go first. The scan does not run `cargo`, so none of the proposed fixes has been compiled.
