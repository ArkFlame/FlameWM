READY

# Dock and taskbar settings spec

Keys are final and implemented in `crates/flamewm-platform/src/settings.rs` (tests in
`crates/flamewm-platform/src/settings/dock_settings_tests.rs`) and mirrored in
`crates/flamewm-api/src/settings_groups.rs`. Existing `taskbarStyle` (classic|floating) is unchanged.

## Keys

| Key | Type | Default | Allowed values | Notes |
| --- | --- | --- | --- | --- |
| `dock.enabled` | bool | `false` | `0`/`1`, `true`/`false` | Dock surface is created only when true. |
| `dock.position` | enum | `bottom` | `bottom`, `left`, `right`, `top` | Left and right use the vertical variant. |
| `dock.iconSize` | integer px | `32` | `24`..`96` | Out of range: parse error (validation). |
| `dock.autoHide` | bool | `false` | bool | Slides the dock out when idle; the pointer on its edge strip reveals it. Missing key in a legacy file = `true` (old behaviour kept). |
| `dock.reserveSpace` | bool | `false` | bool | Reserves the dock's strip in the work area. Only effective while `dock.autoHide` is false. Missing key = `!autoHide`, so legacy files keep their reserve behaviour. |
| `dock.cornerStyle` | enum | `rounded` | `rounded`, `squared` | `squared` = corner radius 0. Rendering is owned by the dock surface. |
| `dock.surfaceCustom` | bool | `false` | bool | When false the dock follows the global surface keys. When true the `dock.surface*` keys below apply. |
| `dock.surfaceColor` | colour | unset | colour | Per-dock override of `surfaceColor`. Read only while `dock.surfaceCustom` is true. |
| `dock.surfaceOpacity` | integer % | unset | 0..100 | Per-dock override of `surfaceOpacity`. |
| `dock.surfaceBlurPercent` | integer % | unset | 0..100 | Per-dock override of `surfaceBlurPercent`. |
| `dock.surfaceOutlineColor` | colour | unset | colour | Per-dock override of `surfaceOutlineColor`. |
| `dock.surfaceOutlineOpacity` | integer % | unset | 0..100 | Per-dock override of `surfaceOutlineOpacity`. |
| `dock.surfaceOutlineWidth` | integer px | unset | see validation | Per-dock override of `surfaceOutlineWidth`. |
| `dock.pinned` | comma list | empty | app ids: `[A-Za-z0-9._-]`, 1..128 bytes, max 32 | Duplicates dropped on load. Invalid id: parse error. Unknown ids kept. |
| `dock.showRunning` | bool | `true` | bool | Unpinned running apps after the separator. |
| `taskbar.enabled` | bool | `true` | bool | False: taskbar panel not created, reserves no work area. |
| `taskbar.fill` | enum | `solid` | `solid`, `gradient` | Surface fill. Gradient uses the surface colour token. |
| `taskbar.gradientEdge` | enum | `auto` | `auto`, `top`, `bottom`, `left`, `right` | Edge the gradient is opaque at, fading inward. `auto` = the taskbar's own edge. |

Default values are never written to the settings file. Bools are written as `true`/`false`.
Enum and list values fail with `InvalidArgument` when unknown.

## Gradient geometry

Opaque at the edge, transparent toward the interior:
- top: `to bottom`; bottom: `to top`; left: `to right`; right: `to left`.
- Pure helper: `flamewm_shell_core::dock::gradient_background(edge, "var(--surface)")`.

## Settings page controls (Terra)

Own page **Dock** (sidebar row `nav-dock`, section `page-dock`, placed after Taskbar). Dock options are not shown on Taskbar.

Visibility is dependent, resolved by `dock_page::dock_rows(enabled, auto_hide, surface_custom)`:
- Toggle "Show dock" -> `dock.enabled`. When off, no other dock control is shown.
- When on: Toggle "Automatically hide" -> `dock.autoHide` (default off).
- Toggle "Reserve space" -> `dock.reserveSpace`, shown only when `dock.autoHide` is off.
- Segmented "Position" (Bottom / Left / Right / Top) -> `dock.position`.
- Slider "Icon size" 24..96 step 4 -> `dock.iconSize` (default 32; no height option).
- Segmented "Corner style" (Rounded / Squared) -> `dock.cornerStyle`.
- Toggle "Show running apps" -> `dock.showRunning`.
- List of pinned apps with remove -> `dock.pinned`.
- Toggle "Custom surface" -> `dock.surfaceCustom`. When on, reveals surface colour, blur, opacity and outline controls that reuse the shared Surfaces widgets and write `dock.surface*`. When off, the dock follows the global surface settings.

Group **Taskbar**:
- Toggle "Show taskbar" -> `taskbar.enabled`. If it is off and `dock.enabled` is off, warn and refuse.
- Segmented "Surface" (Solid / Gradient) -> `taskbar.fill`.
- Segmented "Gradient edge" (Auto / Top / Bottom / Left / Right), enabled only when Surface = Gradient -> `taskbar.gradientEdge`.

The global surface colour stays the taskbar/surface setting. Dock surface overrides are read via `flamewm_api::settings_groups::dock_surface_value`.

## Migration

- Missing `dock.autoHide` in a file parses as `true`, and missing `dock.reserveSpace` parses as `!autoHide`. Existing files keep their behaviour; the new defaults apply only to new files.
- Base rows `dock.autoHide` and `dock.reserveSpace` are always written, so explicit values survive a save.
- No existing dock key is renamed or removed.
