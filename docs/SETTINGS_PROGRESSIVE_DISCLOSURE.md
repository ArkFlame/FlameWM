# Settings progressive disclosure

Status: inventory and classification done. Implementation status is in the last table.

## Rule

- A group with more than 3 related controls gets one `Custom <group>` switch (boolean key).
- Switch off: the controls are hidden and every key in the group resolves to its default.
- Switch on: the controls show. Stored custom values are kept while off (no data loss).
- A single-control setting gets no switch. Groups with 2-3 controls stay direct.
- Migration: an existing config whose group values differ from defaults loads with the switch ON.
  Clean configs load with the switch OFF.

## Inventory (source: `ui/settings.html` `setting-group-title` / `setting-title` rows)

Counts are the rows in each group, including a switch row where one exists.

| Page | Group | Rows | Class | Switch |
| --- | --- | --- | --- | --- |
| theme | Accent | 2 | switch group (existing) | `accentCustom` (exists) |
| theme | Applications | 2 | direct | - |
| theme | Icons | 1 | single | - |
| theme | Cursor | 5 | switch group | proposed `cursorCustom` |
| theme | Advanced | 1 | single | - |
| start-menu | Start icon | 1 | single | - |
| surfaces | Surface | 6 | switch group | proposed `surfaceCustom` |
| surfaces | Window outline | 4 | switch group (existing) | `outlineCustom` (exists) |
| surfaces | Liquid glass | 5 | switch group | proposed `glassCustom` |
| surfaces | Taskbar material | 3 | direct | - |
| surfaces | Window frame | 3 | direct | - |
| surfaces | Advanced | 3 | direct | - |
| animations | General | 2 | direct | - |
| animations | Start menu | 2 | direct | - |
| animations | Open and close | 3 | direct | - |
| animations | Minimize and maximize | 4 | switch group | proposed `minimizeMaximizeCustom` |
| lock-screen | Background | 5 | switch group | proposed `lockBackgroundCustom` |
| lock-screen | Clock and user | 4 | switch group | proposed `lockClockCustom` |
| lock-screen | Locking | 1 | single | - |
| screensaver | Screensaver | 4 | switch group | proposed `screensaverCustom` |
| screensaver | Ribbons | 2 | direct | - |
| desktop | Wallpaper | 3 | direct | - |
| desktop | Desktop | 3 | direct | - |
| desktop | Windows | 5 | switch group | proposed `windowsCustom` |
| desktop | Advanced | 2 | direct | - |
| taskbar | Panel | 4 | switch group | proposed `panelCustom` |
| taskbar | Items | 8 | switch group | proposed `itemsCustom` |
| taskbar | Clock | 3 | direct | - |
| taskbar | Weather | 4 | exempt (feature master `Enable weather` already gates it) | - |
| taskbar | Advanced | 3 | direct | - |
| displays | Selected output | 6 | exempt (runtime per-output state, not defaults) | - |
| displays | Blue light filter | 5 | exempt (`Night Shift` is the master toggle) | - |
| fonts | Interface typography | 4 | switch group | proposed `fontsCustom` |
| assistant | Assistant | 6 | exempt (feature toggles and API key) | - |
| hotkeys | Window navigation / Screenshots and lock | - | exempt (capture rows) | - |

Switch groups total: 13 (11 proposed, 2 existing). Correction: the existing window-outline switch is
`customWindowOutline` (there is no `outlineCustom` key). Members per group, the single source for the
code, are in `crates/flamewm-api/src/settings_groups.rs` (`SWITCH_GROUPS`).

| Group (`name`) | Switch | Members |
| --- | --- | --- |
| accent | `accentCustom` | accent |
| outline | `customWindowOutline` | windowOutlineColor, windowOutlineOpacity, windowOutlineWidth |
| surface | `surfaceCustom` | outlines, surfaceColor, surfaceOpacity, surfaceOutlineColor, surfaceOutlineOpacity, surfaceOutlineWidth |
| glass | `glassCustom` | reduceTransparency, glassOpacityPercent, glassBlurPx |
| cursor | `cursorCustom` | cursorTheme, keyboardLayout, cursorSize, pointerSpeed, pointerRawInput |
| minimizeMaximize | `minimizeMaximizeCustom` | minimizeAnimation(+Enabled), maximizeAnimation(+Enabled) |
| lockBackground | `lockBackgroundCustom` | lockBackgroundMode, lockBackgroundColor, lockBackgroundImage, lockBackgroundBlurPercent, lockBackgroundDimPercent |
| lockClock | `lockClockCustom` | lockClockFormat, lockShowDate, lockShowUsername, avatarImage |
| screensaver | `screensaverCustom` | screensaverMode, screensaverAfterLockMinutes, screensaverDisplayOffMinutes |
| windows | `windowsCustom` | focusFollowsMouse, titlebars, showContentWhileDragging, windowMargin, snapPreviewEnabled |
| panel | `panelCustom` | taskbarHeight, taskbarStyle, taskbarSeparate |
| items | `itemsCustom` | taskbarShowWeather/Volume/Network/Battery, taskbarBatteryOnlyWhenRelevant, taskbarShowClock/Workspaces/DesktopButton |
| fonts | `fontsCustom` | fontFamily, fontBold, textShadow, fontSizeOffset |

## Owners (from source)

- Key list, defaults, load migration: `crates/flamewm-platform/src/settings.rs` (`ProductSettings`, `parse_snapshot`, `DEFAULT_ACCENT`).
- Shared default constant: `crates/flamewm-api/src/settings.rs` (`DEFAULT_ACCENT`).
- Settings UI keys and actions: `crates/flamewm-settings/src/consts.rs`, `actions.rs`, `app_impl_1.rs`.
- Page markup: `ui/settings.html`, `ui/settings.css`.

## Accent default incongruence (fixed)

- Stored default was `#539BFF` (`DEFAULT_ACCENT`), but the picker's blue swatch and `accent_for_swatch("blue")` are `#3DAEE9`.
- `DEFAULT_ACCENT` is now `#3DAEE9`. Regression test: `default_accent_is_the_picker_blue_swatch`.
- Not changed: `ui/flamewm.css` and `ui/settings.css` still hard-code `--accent:#539bff`. Owned by the theme-token worker.

## Implementation status

| Item | Status |
| --- | --- |
| Inventory table | done (this file) |
| Accent default = picker blue | done (Rust constant + test); CSS tokens pending |
| Accent switch with data retained while off | done: load keeps `accent`; `app_impl_1.rs` no longer clears it; `ProductSettings::effective()` resolves for readers |
| Central effective-value resolver per group | done: `flamewm_api::settings_groups` (`group_of`, `is_group_custom`, `effective_value`) and `ProductSettings::effective()` |
| Migration for the 11 proposed groups | done: `parse_snapshot` switches a group on when a member is non-default and the switch key is absent |
| Contract test: every multi-control group has a switch | done: `switch_contract_tests.rs`, `switch_group_tests.rs` |
| Switch rows + collapsible groups in UI | NOT done (UI worker) |
| Settings search finds hidden keys and turns switch on | NOT done (UI worker) |
