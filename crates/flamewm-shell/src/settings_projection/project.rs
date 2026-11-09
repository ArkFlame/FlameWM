//! Full shell surface projection: assembles each surface document from settings.

use super::*;

/// Settings are presentation input only. Panel placement, edge, and size are
/// deliberately absent from this projection and remain service-owned.
pub fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    surface: SurfaceKind,
) -> Result<(), String> {
    let root = match surface {
        SurfaceKind::Panel => "taskbar",
        SurfaceKind::Start => "start-menu",
        SurfaceKind::Launcher => "launcher-popup",
        SurfaceKind::Switcher => "switcher-popup",
        SurfaceKind::TaskMenu => "task-menu",
        SurfaceKind::Media => "media-popup",
        SurfaceKind::Osd => "osd",
        SurfaceKind::Dock => "dock-bar",
    };
    let accent = accent_color(snapshot);
    let font_family = text_setting(snapshot, "fontFamily", DEFAULT_FONT_FAMILY);
    document.font_family(font_family)?;
    document.ui_font_defaults(
        integer_setting(snapshot, "fontSizeOffset", 0) as f32,
        bool_setting(snapshot, "fontBold", false),
    )?;
    document.ui_text_shadow(bool_setting(snapshot, "textShadow", true))?;
    project_accent_tokens(document, snapshot)?;
    // The classic/separate panel overwrites the root border with a transparent one below. Writing
    // the contour first alternated two values on the same node every projection, dirtying the
    // document (and repainting the Dock) each time with an identical final frame.
    let panel_overrides_root = surface == SurfaceKind::Panel && {
        let separate = bool_setting(snapshot, "taskbarSeparate", false);
        let classic = !matches!(
            snapshot.values.get("taskbarStyle"),
            Some(SettingValue::Text(style)) if style == "floating"
        );
        separate || classic
    };
    if !panel_overrides_root {
        project_surface_outline(document, root, snapshot)?;
    }

    match surface {
        SurfaceKind::Panel => {
            // Taskbar capsule fill is the Dock material (Blur Only / Inherit / Custom Tint), not the
            // generic surface pair; reduceTransparency makes it the opaque Dock solid.
            let fill = taskbar_fill(snapshot);
            crate::projection::set_taskbar_surface(fill);
            crate::projection::set_taskbar_border(flamewm_ui_x11::surface_style::surface_outline(
                snapshot,
            ));
            // Separate mode paints the wallpaper (the captured backdrop) across the bar and one
            // capsule per element; the root must not tint the gaps between them.
            let separate = bool_setting(snapshot, "taskbarSeparate", false);
            // Classic unless `taskbarStyle` is exactly `floating`, the strings the platform reads.
            let classic = !matches!(
                snapshot.values.get("taskbarStyle"),
                Some(SettingValue::Text(style)) if style == "floating"
            );
            document.background(
                "taskbar",
                if separate {
                    UiColor { a: 0, ..fill }
                } else {
                    fill
                },
            )?;
            // A transparent fill alone does not suppress the separately projected 1px
            // contour. Only the per-node capsules have outlines in separated mode, and the
            // classic bar replaces the bar contour with the one full-width top edge below.
            if separate || classic {
                document.border("taskbar", UiColor { a: 0, ..fill })?;
            }
            // Classic draws a single 1 px top outline across the panel viewport, so the bar
            // reaches the screen end instead of closing off its own inset border box.
            document.background(
                "taskbar-outline",
                if classic {
                    flamewm_ui_x11::surface_style::surface_outline(snapshot)
                } else {
                    UiColor { a: 0, ..fill }
                },
            )?;
            project_taskbar_gradient(document, snapshot, fill)?;
            crate::projection::set_task_accent(accent);
            // A settings change must also re-apply (or clear) the per-element capsules, so
            // disabling `taskbarSeparate` cannot leave the last projected fills behind.
            crate::projection::reapply_element_backgrounds(
                document,
                taskbar_items(Some(snapshot)),
            )?;
            project_clock_at(document, Local::now(), clock_display(snapshot))?;
            project_start_icon(document, custom_start_icon(snapshot))?;
        }
        SurfaceKind::Start => {
            // Start shares the surfaceColor/surfaceOpacity pair with the taskbar and popups.
            document.background(root, surface_background(snapshot))?;
            document.corner_radius(root, popup_corner_radius(snapshot))?;
            // Opaque interior token: the menu root already carries the surface alpha, so the
            // search field must not apply surfaceOpacity a second time.
            document.background(
                "start-search-box",
                UiColor {
                    r: 0x14,
                    g: 0x16,
                    b: 0x18,
                    a: 0xff,
                },
            )?;
            // No accent outline: the menu's rounded panes cover the border on three sides, so an
            // accent-coloured border only ever showed as a red line down the left edge.
        }
        SurfaceKind::TaskMenu => {
            // Task menu shares the desktop menu frame (ui/shared/menu.css 1px border); no accent outline.
        }
        SurfaceKind::Launcher | SurfaceKind::Switcher => {
            // Launcher and switcher share the Start surface pair so the popups match in every theme.
            document.background(root, surface_background(snapshot))?;
            document.corner_radius(root, popup_corner_radius(snapshot))?;
        }
        SurfaceKind::Media => {
            // Root contour is always the shared Surfaces border, not the accent.
        }
        SurfaceKind::Osd => {
            // Volume OSD card: the shared surface fill; its outline and pill radius stay in osd.html.
            document.background(root, surface_background(snapshot))?;
        }
        SurfaceKind::Dock => {
            // Pill fill is the taskbar material; the outline comes from project_surface_outline above.
            // The top highlight is the same outline colour at half alpha, so it never drifts from it.
            document.background(root, taskbar_fill(snapshot))?;
            let outline = flamewm_ui_x11::surface_style::surface_outline(snapshot);
            document.background(
                "dock-highlight",
                UiColor {
                    a: outline.a / 2,
                    ..outline
                },
            )?;
        }
    }
    Ok(())
}

/// The taskbar fill from the Dock material (`dock_fill`). The Dock pill paints the same colour.
pub(super) fn taskbar_fill(snapshot: &SettingsSnapshot) -> UiColor {
    let dock = flamewm_ui_core::surface::dock_fill(snapshot);
    UiColor {
        r: (dock.rgb >> 16) as u8,
        g: (dock.rgb >> 8) as u8,
        b: dock.rgb as u8,
        a: dock.alpha,
    }
}

/// Gradient taskbar fill (`taskbar.fill=gradient`): the bar background goes transparent here.
/// The fading image is built per panel edge by the shell (`ShellSurfaces::apply_taskbar_gradient`),
/// because placement is service-owned and absent from settings. Solid hides that image.
fn project_taskbar_gradient(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    fill: UiColor,
) -> Result<(), String> {
    let gradient = matches!(
        snapshot.values.get("taskbar.fill"),
        Some(SettingValue::Text(fill_mode)) if fill_mode == "gradient"
    );
    if !gradient {
        return document.visible("taskbar-gradient", false);
    }
    document.background("taskbar", UiColor { a: 0, ..fill })
}
