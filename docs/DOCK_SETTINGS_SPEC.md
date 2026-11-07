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
| `dock.iconSize` | integer px | `48` | `24`..`96` | Out of range: parse error (validation). |
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

Group **Dock**:
- Toggle "Show dock" -> `dock.enabled`.
- Segmented "Position" (Bottom / Left / Right / Top) -> `dock.position`.
- Slider "Icon size" 24..96 step 4 -> `dock.iconSize`.
- Toggle "Show running apps" -> `dock.showRunning`.
- List of pinned apps with remove -> `dock.pinned`.

Group **Taskbar**:
- Toggle "Show taskbar" -> `taskbar.enabled`. If it is off and `dock.enabled` is off, warn and refuse.
- Segmented "Surface" (Solid / Gradient) -> `taskbar.fill`.
- Segmented "Gradient edge" (Auto / Top / Bottom / Left / Right), enabled only when Surface = Gradient -> `taskbar.gradientEdge`.

No new colour controls. The surface colour stays the existing taskbar colour setting.
