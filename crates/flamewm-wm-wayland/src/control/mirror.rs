//! Compositor-state mirror: builds the engine's read snapshot and pushes settings into the model.

use super::*;

pub(super) fn toplevel_metadata(state: &State, id: WindowId) -> (String, String) {
    if let Some(top) = state.toplevel(id) {
        return compositor::with_states(top.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| {
                    let data = data
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (
                        data.title.clone().unwrap_or_default(),
                        data.app_id.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        });
    }
    state
        .x11
        .iter()
        .find(|(wid, _)| *wid == id)
        .map(|(_, window)| (window.title(), window.class()))
        .unwrap_or_default()
}

/// Minutes after local midnight for the Night Shift schedule.
pub(super) fn now_minutes() -> u32 {
    use chrono::{Local, Timelike};
    let now = Local::now();
    now.hour() * 60 + now.minute()
}

pub(super) fn to_api_rect(rect: crate::model::Rect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.w,
        height: rect.h,
    }
}

/// Reads the compositor state into the engine's mirror.
///
/// `workspace_revision` is passed in (not derived) so the engine can advance it when the topology
/// or the visible workspace moves; deriving it here would make the value depend on the observation
/// point instead of the change.
pub(super) fn build_mirror(state: &State, workspace_revision: u64) -> Mirror {
    let focused = state.focused;
    let windows = state
        .model
        .managed()
        .iter()
        .map(|window| {
            let (title, app_id) = toplevel_metadata(state, window.id);
            let minimized = window.minimized;
            let window_state = match (minimized, window.mode) {
                (true, _) => WindowState::Minimized,
                (false, WindowMode::Maximized) => WindowState::Maximized,
                (false, WindowMode::Fullscreen) => WindowState::Fullscreen,
                (false, WindowMode::Normal | WindowMode::Snapped(_)) => WindowState::Normal,
            };
            WindowSnapshot {
                reference: WindowRef {
                    id: window.id,
                    generation: 1,
                },
                title,
                app_id: DesktopAppId::new(app_id),
                outer_geometry: to_api_rect(window.frame()),
                restore_geometry: to_api_rect(window.restore),
                state: window_state,
                sticky: false,
                focused: focused == Some(window.id),
                workspace: WorkspaceRef {
                    index: window.workspace as i32,
                    revision: 1,
                },
                output: WaylandDesktop::output_id(),
                state_generation: 1,
            }
        })
        .collect();
    let pointer = (state.pointer.x as i32, state.pointer.y as i32);
    Mirror {
        windows,
        output: to_api_rect(state.output_rect()),
        work_area: to_api_rect(state.work_area()),
        workspace_count: state.model.workspace_count(),
        active_workspace: state.model.current_workspace(),
        workspace_revision,
        pointer,
    }
}

impl Service {
    /// Refreshes the mirror from `state` and reports whether it changed.
    ///
    /// The workspace revision advances here, in the same place the visible workspace and the
    /// topology are observed: a switch, a count change, or a plan that moves windows between
    /// workspaces is an announcement the pager must receive, so the revision is part of the mirror
    /// comparison instead of a value a reader derives on demand.
    pub(super) fn sync(&self, state: &State) -> bool {
        let previous = self.mirror.borrow().clone();
        let next = build_mirror(state, self.workspace_revision.get());
        let workspace_moved = previous.workspace_count != next.workspace_count
            || previous.active_workspace != next.active_workspace;
        if workspace_moved {
            self.workspace_revision
                .set(self.workspace_revision.get() + 1);
        }
        let next = build_mirror(state, self.workspace_revision.get());
        let changed = *self.mirror.borrow() != next;
        if changed {
            *self.mirror.borrow_mut() = next;
            self.publish_pending.set(true);
        }
        changed
    }

    /// Pushes `titlebars` and `windowMargin` from the committed settings into the model.
    ///
    /// The values are read the way X11 `live_settings` reads them: a missing or ill-typed value
    /// keeps the product default, so a hand-edited file cannot put the model in an odd state.
    pub(super) fn sync_chrome_settings(&self, state: &mut State) {
        let snapshot = self.host.borrow().settings_snapshot();
        let (titlebars, window_margin) = chrome_settings(&snapshot);
        if state.model.titlebars() != titlebars || state.model.window_margin() != window_margin {
            state.model.configure(titlebars, window_margin);
            state.request_redraw();
        }
        let (edge_snap, edge_px) = edge_snap_settings(&snapshot);
        if state.model.edge_snap() != (edge_snap, edge_px) {
            state.model.configure_edge_snap(edge_snap, edge_px);
        }
        // The window outline comes from the same shared owner X11 reads, so both backends show one
        // `windowOutlineColor` / `windowOutlineOpacity` / `outlines` ring style.
        // Light titlebar material and its ink follow the same `appearance` the other surfaces use.
        // Cached bars bake both in, so an appearance change drops them.
        let appearance = flamewm_ui_core::glass::appearance_from_settings(&snapshot);
        let fill = flamewm_ui_core::surface::titlebar_fill(&snapshot);
        let blur = flamewm_ui_core::surface::surface_blur_percent(&snapshot);
        let light_changed = crate::render::set_titlebar_light(appearance.light);
        let fill_changed = crate::render::set_glass_fill(
            fill.rgb,
            fill.alpha,
            blur,
            appearance.reduce_transparency,
        );
        if light_changed || fill_changed {
            state.decorations.borrow_mut().invalidate_bars();
            state.request_redraw();
        }
        let stroke = flamewm_ui_core::surface::window_outline_stroke(&snapshot);
        let outline_changed =
            state
                .decorations
                .borrow_mut()
                .set_outline(stroke.rgb, stroke.alpha, stroke.width);
        if outline_changed {
            state.request_redraw();
        }
        // The workspace topology is a product decision persisted beside the settings file, and X11
        // already honours it. Adopting it here is what makes the pager's cells match the session
        // instead of a compiled-in default.
        if let Some(count) = self.host.borrow().workspace_count() {
            let moved = state.model.set_count(count);
            state.request_redraw();
            if moved {
                state.refocus();
            }
        }
        // Blue light filter: the compositor has no DRM gamma ramp through Smithay, so it paints the
        // shared amber veil instead. One comparison per turn keeps this off the paint path.
        let blue_light = resolve_at(
            flamewm_api::blue_light::BlueLightConfig::from_snapshot(
                &self.host.borrow().settings_snapshot(),
            ),
            now_minutes(),
        )
        .intensity;
        if state.blue_light_intensity != blue_light {
            state.blue_light_intensity = blue_light;
            state.request_redraw();
        }
    }
}
