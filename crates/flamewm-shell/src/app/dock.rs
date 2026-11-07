//! Dock surface lifecycle and clicks. The surface exists only while `dock.enabled`
//! is true; it is projected from the window snapshot and follows settings changes.

use super::*;
use flamewm_api::PanelEdge;

use flamewm_ui_x11::{ActionPhase, SurfaceEvent};
use flamewm_shell::dock::{self, DockSettings};
use flamewm_shell::icon_loader::IconTarget;
use flamewm_shell_core::dock::{drag_off_removes, move_pinned, slot_at, Edge};
use crate::notification_center::is_escape_key;
use flamewm_ui_document::UiDocumentAccess;

/// Pointer distance (surface px) before a pressed pinned slot becomes a drag.
const DRAG_START_PX: f32 = 6.0;
/// Pointer distance off the bar (surface px) that unpins a dragged slot.
const DRAG_OFF_PX: f32 = 40.0;

/// A pinned slot pressed with the primary button; becomes a drag past `DRAG_START_PX`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DockDrag {
    pub(crate) from: usize,
    pub(crate) start: (f32, f32),
    pub(crate) pos: (f32, f32),
    pub(crate) active: bool,
}

/// Last projected dock state; an identical state skips the document write.
pub(crate) type DockSync = (dock::DockFrame, (i32, i32, u32, u32), Vec<String>, Vec<flamewm_shell_core::dock::DockEntry>);

fn dock_edge(edge: PanelEdge) -> Edge {
    match edge {
        PanelEdge::Bottom => Edge::Bottom,
        PanelEdge::Top => Edge::Top,
        PanelEdge::Left => Edge::Left,
        PanelEdge::Right => Edge::Right,
    }
}

impl ShellLoop {
    /// Creates, moves, projects, or destroys the dock surface to match the settings and windows.
    pub(crate) fn sync_dock(&mut self, runtime: &mut SurfaceRuntime) {
        let settings: DockSettings = dock::dock_settings(self.settings_cache.as_ref());
        if !settings.enabled {
            self.dock_entries.clear();
            self.dock_last = None;
            self.close_dock_menu(runtime);
            if let Some(surface) = self.dock_surface.take() {
                let _ = runtime.destroy(surface);
            }
            return;
        }
        let snapshot = self.shell.snapshot();
        let Some(panel) = snapshot.panels.panels.first() else {
            return;
        };
        let Some(output) = snapshot.displays.outputs.iter().find(|o| o.id == panel.output) else {
            return;
        };
        let layout = dock::layout_for(&settings, &snapshot.windows);
        // Pinned apps missing from the shell snapshot fall back to the same desktop catalog the launcher uses.
        if self.dock_catalog.is_none() {
            self.dock_catalog = Some(
                flamewm_applications::ApplicationCatalog::discover()
                    .map(|catalog| catalog.all())
                    .unwrap_or_default(),
            );
        }
        let catalog: &[flamewm_api::applications::DesktopApplication] =
            self.dock_catalog.as_deref().unwrap_or(&[]);
        let lookup = |id: &str| {
            snapshot
                .applications
                .iter()
                .chain(catalog.iter())
                .find(|app| app.id.as_str() == id)
        };
        let running: Vec<bool> = layout.entries.iter().map(|entry| entry.running).collect();
        let frame = dock::dock_frame(&running, layout.separator_after, settings.icon_size, settings.edge);
        let scale = flamewm_shell::runtime::panel_ui_scale(output.shell_scale_percent);
        // The taskbar keeps its strip when the dock shares its edge.
        let inset = if dock_edge(panel.edge) == settings.edge {
            if settings.edge.is_vertical() {
                panel.geometry.width
            } else {
                panel.geometry.height
            }
        } else {
            0
        };
        let geometry = output.geometry;
        let rect = dock::place(
            &frame,
            (geometry.x, geometry.y, geometry.width, geometry.height),
            scale,
            inset,
        );
        let surface = match self.dock_surface {
            Some(surface) => surface,
            None => match flamewm_shell::runtime::create_dock_surface(runtime, rect, scale) {
                Ok(surface) => {
                    self.dock_surface = Some(surface);
                    surface
                }
                Err(error) => {
                    eprintln!("flamewm-shell: dock surface create failed: {error}");
                    return;
                }
            },
        };
        let names: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| {
                lookup(&entry.app_id)
                    .map_or_else(|| entry.app_id.clone(), |app| app.name.clone())
            })
            .collect();
        let state: DockSync = (frame.clone(), rect, names.clone(), layout.entries.clone());
        if self.dock_last.as_ref() == Some(&state) {
            return;
        }
        let icon_px = (settings.icon_size * scale).round() as u32;
        let icon_names: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| {
                lookup(&entry.app_id)
                    .map_or_else(String::new, |app| app.icon_name.clone())
            })
            .collect();
        async_projection::bump_dock_view_epoch();
        if let Some(loader) = self.icon_loader.as_mut() {
            for (index, name) in icon_names.iter().enumerate() {
                let target = IconTarget::DockSlot(index as u8);
                let generation = loader.generation();
                loader.request(target, name, icon_px, icon_px);
                async_projection::bind_icon_request(target, name, icon_px, icon_px, generation);
            }
        }
        let _ = runtime.move_resize(surface, rect.0, rect.1, rect.2, rect.3);
        if let Err(error) = runtime.with_document(surface, |document| {
            dock::project_dock(document, &frame, &names)?;
            for index in 0..icon_names.len() {
                async_projection::apply_cached_binding(document, IconTarget::DockSlot(index as u8), icon_px)?;
            }
            Ok::<(), String>(())
        }) {
            eprintln!("flamewm-shell: dock projection failed: {error:?}");
        }
        let shown = if layout.entries.is_empty() {
            runtime.hide(surface)
        } else {
            runtime.show(surface)
        };
        let _ = shown;
        let _ = runtime.redraw(surface);
        self.dock_entries = layout.entries;
        self.dock_last = Some(state);
    }

    /// Writes the pinned list to `dock.pinned` through the settings service.
    fn persist_dock_pins(&mut self, ids: Vec<String>) {
        self.submit(ControlRequest::ApplySettings(
            flamewm_api::settings::SettingsTransaction {
                expected_revision: self.settings_revision,
                changes: vec![flamewm_api::settings::SettingsChange {
                    key: "dock.pinned".to_owned(),
                    value: Some(flamewm_api::settings::SettingValue::Text(ids.join(","))),
                }],
                reset_section: None,
            },
        ));
    }

    /// Reorders or unpins the dragged pinned slot, then persists the new list.
    fn finish_dock_drag(&mut self, drag: DockDrag) {
        let Some((frame, ..)) = self.dock_last.as_ref() else {
            return;
        };
        let Some(strip) = frame.slots.first().copied() else {
            return;
        };
        let vertical = frame.edge.is_vertical();
        let (along, cross) = if vertical {
            (drag.pos.1 - strip.y, drag.pos.0 - frame.bar.x)
        } else {
            (drag.pos.0 - strip.x, drag.pos.1 - frame.bar.y)
        };
        let bar_extent = if vertical { frame.bar.w } else { frame.bar.h };
        let pitch = if frame.slots.len() > 1 {
            if vertical {
                frame.slots[1].y - strip.y
            } else {
                frame.slots[1].x - strip.x
            }
        } else if vertical {
            strip.h
        } else {
            strip.w
        };
        let pinned_count = self.dock_entries.iter().take_while(|e| e.pinned).count();
        let mut ids: Vec<String> = self
            .dock_entries
            .iter()
            .take(pinned_count)
            .map(|e| e.app_id.clone())
            .collect();
        if drag_off_removes(cross, bar_extent, DRAG_OFF_PX) {
            if drag.from < ids.len() {
                ids.remove(drag.from);
            }
        } else {
            let target = slot_at(along, pitch, pinned_count);
            if !move_pinned(&mut ids, drag.from, target) || target == drag.from {
                return;
            }
        }
        self.persist_dock_pins(ids);
    }

    /// Press, drag, and release on dock slots, then clicks that launch or focus.
    pub(crate) fn route_dock_event(
        &mut self,
        runtime: &mut SurfaceRuntime,
        event: &SurfaceEvent,
    ) -> Result<(), UiBackendError> {
        let slot = event
            .action
            .action
            .strip_prefix("dock.slot.")
            .and_then(|n| n.parse::<usize>().ok());
        let (x, y) = (event.action.x, event.action.y);
        let pinned_count = self.dock_entries.iter().take_while(|e| e.pinned).count();
        match event.action.phase {
            ActionPhase::Press => {
                self.close_dock_menu(runtime);
                self.dock_drag = slot
                    .filter(|n| event.action.button == 1 && *n >= 1 && *n <= pinned_count)
                    .map(|n| DockDrag {
                        from: n - 1,
                        start: (x, y),
                        pos: (x, y),
                        active: false,
                    });
                return Ok(());
            }
            ActionPhase::Motion => {
                if let Some(drag) = self.dock_drag.as_mut() {
                    drag.pos = (x, y);
                    let (dx, dy) = (x - drag.start.0, y - drag.start.1);
                    if dx * dx + dy * dy >= DRAG_START_PX * DRAG_START_PX {
                        drag.active = true;
                    }
                }
                return Ok(());
            }
            ActionPhase::Release => {
                // The backend emits press only for the primary button; secondary opens on release.
                if event.action.button == 3 {
                    self.close_dock_menu(runtime);
                    if let Some(n) = slot.filter(|n| *n >= 1 && *n <= self.dock_entries.len()) {
                        if event.action.inside {
                            self.open_dock_menu(runtime, n - 1);
                        }
                    }
                    return Ok(());
                }
                if let Some(mut drag) = self.dock_drag.take() {
                    if drag.active {
                        drag.pos = (x, y);
                        self.finish_dock_drag(drag);
                        return Ok(());
                    }
                }
            }
            ActionPhase::Hover => {}
        }
        if event.action.phase == ActionPhase::Hover {
            // The open menu owns the space above the bar, so no hover label shows.
            let hovered = slot
                .filter(|_| event.action.inside && self.dock_menu_slot.is_none())
                .map(|n| n - 1)
                .filter(|index| *index < self.dock_entries.len());
            if hovered != self.dock_hover {
                self.dock_hover = hovered;
                if let Some(surface) = self.dock_surface {
                    let _ = runtime.with_document(surface, |document| {
                        dock::project_hover(document, hovered)
                    });
                    let _ = runtime.redraw(surface);
                }
            }
            return Ok(());
        }
        if event.action.phase != ActionPhase::Release
            || !event.action.inside
            || event.action.button != 1
        {
            return Ok(());
        }
        let Some(slot) = slot else {
            return Ok(());
        };
        let Some(entry) = self.dock_entries.get(slot.wrapping_sub(1)) else {
            return Ok(());
        };
        let request = dock::click_request(&entry.app_id, &self.shell.snapshot().windows);
        self.submit(request);
        Ok(())
    }
}


impl ShellLoop {
    /// Hides the right-click menu. The surface stays alive for the next open.
    pub(crate) fn close_dock_menu(&mut self, runtime: &mut SurfaceRuntime) {
        if let Some(menu) = self.dock_menu {
            let _ = runtime.ungrab_pointer(menu);
            let _ = runtime.hide(menu);
        }
        self.dock_menu_slot = None;
        self.dock_menu_items.clear();
    }

    /// Shows the menu beside dock slot `index` on the dock's edge.
    pub(crate) fn open_dock_menu(&mut self, runtime: &mut SurfaceRuntime, index: usize) {
        let Some((frame, rect, ..)) = self.dock_last.as_ref() else {
            return;
        };
        let Some(entry) = self.dock_entries.get(index) else {
            return;
        };
        let Some(slot) = frame.slots.get(index).copied() else {
            return;
        };
        let items = flamewm_shell_core::dock::menu_items(entry.pinned, entry.running);
        let scale = rect.2 as f32 / frame.width.max(1.0);
        let (w, h) = (180.0 * scale, (items.len() as f32 * 28.0 + 10.0) * scale);
        let (w, h) = (w.round() as i32, h.round() as i32);
        let bar = frame.bar;
        let (x, y) = match frame.edge {
            Edge::Bottom => (rect.0 + (slot.x * scale) as i32, rect.1 + ((bar.y * scale) as i32) - h - 4),
            Edge::Top => (rect.0 + (slot.x * scale) as i32, rect.1 + (((bar.y + bar.h) * scale) as i32) + 4),
            Edge::Left => (rect.0 + (((bar.x + bar.w) * scale) as i32) + 4, rect.1 + (slot.y * scale) as i32),
            Edge::Right => (rect.0 + ((bar.x * scale) as i32) - w - 4, rect.1 + (slot.y * scale) as i32),
        };
        let geometry = (x, y, w.max(1) as u32, h.max(1) as u32);
        let menu = match self.dock_menu {
            Some(menu) => {
                let _ = runtime.move_resize(menu, geometry.0, geometry.1, geometry.2, geometry.3);
                menu
            }
            None => match flamewm_shell::runtime::create_dock_menu_surface(runtime, geometry) {
                Ok(menu) => {
                    self.dock_menu = Some(menu);
                    menu
                }
                Err(error) => {
                    eprintln!("flamewm-shell: dock menu create failed: {error}");
                    return;
                }
            },
        };
        let labels: Vec<&str> = items.iter().map(|item| dock::menu_label(*item)).collect();
        let _ = runtime.with_document(menu, |document| {
            for n in 1..=4usize {
                let item = format!("dock-menu-{n}");
                match labels.get(n - 1) {
                    Some(label) => {
                        document.visible(&item, true)?;
                        document.text_ref(&format!("{item}-label"), label)?;
                    }
                    None => document.visible(&item, false)?,
                }
            }
            Ok::<(), String>(())
        });
        let _ = runtime.show(menu);
        let _ = runtime.redraw(menu);
        // Grab so a press outside the menu arrives as `surface.outside.release`.
        if let Err(error) = runtime.grab_pointer(menu) {
            eprintln!("flamewm-shell: dock menu pointer grab refused: {error:?}");
        }
        self.dock_menu_slot = Some(index);
        if let Some(dock_surface) = self.dock_surface {
            self.dock_hover = None;
            let _ = runtime.with_document(dock_surface, |document| dock::project_hover(document, None));
            let _ = runtime.redraw(dock_surface);
        }
        self.dock_menu_items = items;
    }

    /// Runs the picked menu item for the slot the menu was opened on.
    pub(crate) fn route_dock_menu_event(
        &mut self,
        runtime: &mut SurfaceRuntime,
        event: &SurfaceEvent,
    ) -> Result<(), UiBackendError> {
        // Escape or a press outside the menu dismisses it without an action.
        if is_escape_key(&event.action.action, event.action.text.as_deref())
            || (event.action.phase == ActionPhase::Release
                && !event.action.inside
                && event.action.action == "surface.outside.release")
        {
            self.close_dock_menu(runtime);
            return Ok(());
        }
        if event.action.phase != ActionPhase::Release
            || !event.action.inside
            || event.action.button != 1
        {
            return Ok(());
        }
        let pick = event
            .action
            .action
            .strip_prefix("dock.menu.")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| self.dock_menu_items.get(n.wrapping_sub(1)).copied());
        let slot = self.dock_menu_slot;
        self.close_dock_menu(runtime);
        let (Some(action), Some(slot)) = (pick, slot) else {
            return Ok(());
        };
        let Some(entry) = self.dock_entries.get(slot).cloned() else {
            return Ok(());
        };
        let pinned_ids: Vec<String> = self
            .dock_entries
            .iter()
            .take_while(|e| e.pinned)
            .map(|e| e.app_id.clone())
            .collect();
        match action {
            flamewm_shell_core::dock::DockMenuAction::KeepInDock => {
                let mut ids = pinned_ids;
                if !ids.contains(&entry.app_id) && ids.len() < flamewm_shell_core::dock::MAX_PINNED {
                    ids.push(entry.app_id.clone());
                    self.persist_dock_pins(ids);
                }
            }
            flamewm_shell_core::dock::DockMenuAction::RemoveFromDock => {
                let ids: Vec<String> = pinned_ids.into_iter().filter(|id| *id != entry.app_id).collect();
                self.persist_dock_pins(ids);
            }
            flamewm_shell_core::dock::DockMenuAction::Open => {
                self.submit(ControlRequest::LaunchApplication {
                    app: flamewm_api::DesktopAppId::new(entry.app_id.as_str()),
                    options: flamewm_api::applications::ApplicationLaunchOptions::default(),
                });
            }
            flamewm_shell_core::dock::DockMenuAction::CloseWindows => {
                let windows: Vec<flamewm_api::WindowRef> = self
                    .shell
                    .snapshot()
                    .windows
                    .iter()
                    .filter(|w| !w.skip_taskbar && w.app_id.as_str() == entry.app_id)
                    .map(|w| w.reference.clone())
                    .collect();
                for window in windows {
                    self.submit(ControlRequest::CloseWindow(window));
                }
            }
        }
        Ok(())
    }
}
