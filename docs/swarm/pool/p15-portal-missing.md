# Unit p15-portal-missing
STATUS: PARTIAL

crates/flamewm-integrations-linux: xdg-desktop-portal absent (file chooser, screenshot, settings): skip with one record and a clear user-visible message where the user triggered it; audit and fix.

## Result
STARTED

Production edits: 0 of 3 allowed. The requested absent-portal degrade path is already present in the shared checkout, so no further code change was warranted. No cargo/build/test/git was run in this unit; every row below is UNBUILT.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-integrations-linux/src/portal_file_chooser.rs:29-51,133-142` | Audit: absent-portal handling already exists. `PORTAL_ABSENT_MESSAGE` names `xdg-desktop-portal`; `PORTAL_ABSENT_RECORDED` emits exactly one `portal.absent` debug event per process; `is_portal_absent` maps only `ServiceUnknown` for `org.freedesktop.portal.Desktop`, while every other D-Bus error keeps its original cause. | None - already in tree - UNBUILT | Low: unchanged behavior; valid-portal and non-absent error paths are untouched. |
| `crates/flamewm-integrations-linux/src/portal_file_chooser.rs:462-488` | Existing inline test asserts the one clear message, the `xdg-desktop-portal` wording, and that timeout/other-service errors are not misclassified. | None - UNBUILT | None; test-only, not executed here. |
| `crates/flamewm-settings/src/file_chooser.rs:79-82`, `crates/flamewm-settings/src/app_impl_1.rs:496-541,74-76`, `crates/flamewm-settings/src/theme_picker.rs:408-429` | Trigger-point audit: wallpaper/lock/avatar/start-icon chooser and theme install map `FlameError::to_string()` into `completion_errors`, which `report_error` shows inline in the Settings UI while authoritative snapshots stay intact. The message is therefore shown where the user triggered the action. | None - UNBUILT | None; no UI or visual change. |
| `crates/flamewm-screenshot/src/portal.rs:24-35`; workspace grep for `org.freedesktop.portal.Screenshot`, `org.freedesktop.portal.Settings`, `PORTAL_SERVICE` | Screenshot is an `org.freedesktop.impl.portal.Screenshot` provider, not a portal client; no in-workspace client of the Screenshot or Settings portal exists, so those categories have no absent-portal trigger point in `flamewm-integrations-linux`. Only the FileChooser client path needed the fix. | None - UNBUILT | None; static audit only. |
| `docs/DEPENDENCIES.md:73`, `packaging/arch/PKGBUILD:65`, `docs/swarm/pool/p03-notify-no-dbus.md:10-14` | Reference-first: the portal frontend is optional (`Recommends`, `No portal dialogs`); xdg-desktop-portal-kde packaging notes and the p03 one-record/degrade precedent agree with skip-and-continue. No vendor dependency introduced. | None - UNBUILT | None. |

PARTIAL - Audit is complete. The requested behavior (skip once with one diagnostic record and a clear, user-visible message at the trigger point) is already implemented for the only portal client in `flamewm-integrations-linux`: the wallpaper/theme `FileChooser` path records `portal.absent` once and returns a message naming `xdg-desktop-portal`, which Settings shows inline. Screenshot and Settings portals have no client call sites in this crate (screenshot is an impl backend), so there was nothing further to fix; zero production edits were made rather than duplicating or widening existing work. Nothing was built or tested because the brief forbids cargo/builds, so all findings are UNBUILT and the orchestrator must verify the batch build. Residual risk is limited to the unchanged `ServiceUnknown`-based detection, which does not reclassify a timeout or another service's failure.
