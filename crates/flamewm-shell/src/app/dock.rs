//! Dock surface lifecycle and clicks. The surface exists only while `dock.enabled`
//! is true; it is projected from the window snapshot and follows settings changes.

use super::*;
use flamewm_api::PanelEdge;

use crate::notification_center::is_escape_key;
use flamewm_shell::dock::{self, DockSettings};
use flamewm_shell::icon_loader::IconTarget;
use flamewm_shell_core::autohide::{AutoHide, PointerSurface};
use flamewm_shell_core::dock::{
    drag_off_removes, hover_scaled_rect, move_pinned, slot_at, Edge, HOVER_SCALE, TRASH_SLOT_ID,
};
use flamewm_ui_document::UiDocumentAccess;
use flamewm_ui_x11::{ActionPhase, DockEdge, SurfaceEvent};

/// Pointer distance (surface px) before a pressed pinned slot becomes a drag.
const DRAG_START_PX: f32 = 6.0;
/// Pointer distance off the bar (surface px) that unpins a dragged slot.
const DRAG_OFF_PX: f32 = 48.0;

/// A pinned slot pressed with the primary button; becomes a drag past `DRAG_START_PX`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DockDrag {
    pub(crate) from: usize,
    pub(crate) start: (f32, f32),
    pub(crate) pos: (f32, f32),
    pub(crate) active: bool,
}

/// Last projected dock state; an identical state skips the document write.
/// Device rect, opacity and blur of the backdrop installed on the Dock surface.
type DockBackdropKey = ((i32, i32, u32, u32), u8, u8);

pub(crate) type DockSync = (
    dock::DockFrame,
    (i32, i32, u32, u32),
    Vec<String>,
    Vec<flamewm_shell_core::dock::DockEntry>,
);

/// Dock surface placement state. Each field caches the last value written, so a tick only
/// talks to the window system when something changed.
#[derive(Default)]
pub(crate) struct DockStage {
    /// Reveal strip surface: mapped only while the dock is hidden.
    pub(crate) reveal: Option<flamewm_ui_x11::SurfaceHandle>,
    pub(crate) autohide: Option<AutoHide>,
    placed: Option<(i32, i32, u32, u32)>,
    reveal_placed: Option<(i32, i32, u32, u32)>,
    /// `(dock mapped, reveal mapped)`.
    mapped: Option<(bool, bool)>,
    strut: Option<((i32, i32, u32, u32), bool)>,
    /// Backdrop last installed as `(device rect, opacity, blur)`; a recapture happens only when one changes.
    backdrop: Option<DockBackdropKey>,
    /// Hover label popup, created on first hover and hidden otherwise.
    pub(crate) label: Option<flamewm_ui_x11::SurfaceHandle>,
    /// Text and root position the label popup last showed; `None` while hidden. A tick with the
    /// same label writes nothing, so the popup is not re-mapped and repainted every frame.
    label_shown: Option<(String, (i32, i32))>,
}

/// Inputs to one placement pass, grouped to keep the call readable.
struct DockPlacement {
    placed: (i32, i32, u32, u32),
    output: (i32, i32, i32, i32),
    scale: f32,
    auto_hide: bool,
    /// False when the dock has no entries; it then stays unmapped, reveal included.
    shown: bool,
    model: AutoHide,
}

fn strut_edge(edge: Edge) -> DockEdge {
    match edge {
        Edge::Bottom => DockEdge::Bottom,
        Edge::Top => DockEdge::Top,
        Edge::Left => DockEdge::Left,
        Edge::Right => DockEdge::Right,
    }
}

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
    /// Auto-hide slides the dock toward its edge; once fully hidden, only the reveal strip is mapped.
    pub(crate) fn sync_dock(&mut self, runtime: &mut SurfaceRuntime) {
        let settings: DockSettings = dock::dock_settings(self.settings_cache.as_ref());
        if !settings.enabled {
            self.dock_entries.clear();
            self.dock_last = None;
            self.close_dock_menu(runtime);
            if let Some(surface) = self.dock_surface.take() {
                let _ = runtime.destroy(surface);
            }
            if let Some(reveal) = self.dock_stage.reveal.take() {
                let _ = runtime.destroy(reveal);
            }
            self.dock_stage = DockStage::default();
            return;
        }
        if self.dock_last.is_none() {
            // A rebuild reads the Trash folder once; later changes re-read it only on reveal.
            self.dock_trash_full =
                flamewm_shell_core::dock::trash_has_items(&dock::trash_data_home());
        }
        let snapshot = self.shell.snapshot();
        let Some(panel) = snapshot.panels.panels.first() else {
            return;
        };
        let Some(output) = snapshot
            .displays
            .outputs
            .iter()
            .find(|o| o.id == panel.output)
        else {
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
        let frame = dock::dock_frame(
            &running,
            layout.separator_after,
            layout.trash_separator_after,
            settings.icon_size,
            settings.edge,
        );
        let scale = flamewm_shell::runtime::panel_ui_scale(output.shell_scale_percent);
        // The taskbar keeps its strip when the dock shares its edge, unless `taskbar.enabled` is off.
        let taskbar_shown = self.settings_cache.as_ref().is_none_or(taskbar_enabled);
        let inset = if taskbar_shown && dock_edge(panel.edge) == settings.edge {
            if settings.edge.is_vertical() {
                panel.geometry.width
            } else {
                panel.geometry.height
            }
        } else {
            0
        };
        let geometry = output.geometry;
        let output_rect = (geometry.x, geometry.y, geometry.width, geometry.height);
        let placed = dock::place(&frame, output_rect, scale, inset);
        let now = std::time::Instant::now();
        let mut model = self
            .dock_stage
            .autohide
            .unwrap_or_else(|| AutoHide::new(settings.auto_hide, now));
        // Covered mode (`dock.autoHideCovered`): hide only while a visible window on this output overlaps the dock.
        let covered = snapshot.windows.iter().any(|window| {
            window.is_visible()
                && !window.skip_taskbar
                && window.output == panel.output
                && flamewm_shell_core::dock::rect_overlaps(
                    (
                        window.outer_geometry.x,
                        window.outer_geometry.y,
                        window.outer_geometry.width,
                        window.outer_geometry.height,
                    ),
                    placed,
                )
        });
        let hide = settings.auto_hide && (!settings.auto_hide_covered || covered);
        model.set_auto_hide(hide, now);
        model.set_fullscreen(snapshot.windows.iter().any(|window| {
            window.state == flamewm_api::window::WindowState::Fullscreen
                && window.output == panel.output
        }));
        model.set_pinned_open(
            self.dock_drag.is_some() || self.dock_menu_slot.is_some(),
            now,
        );
        model.step(now);
        self.dock_stage.autohide = Some(model);
        let surface = match self.dock_surface {
            Some(surface) => surface,
            None => match flamewm_shell::runtime::create_dock_surface(runtime, placed, scale) {
                Ok(surface) => {
                    self.dock_surface = Some(surface);
                    // A freshly created Dock paints the current taskbar material and outline at once.
                    if let Some(settings) = self.settings_cache.as_ref() {
                        let _ = runtime.with_document(surface, |document| {
                            crate::entry::settings_projection::project(
                                document,
                                settings,
                                crate::entry::settings_projection::SurfaceKind::Dock,
                            )
                        });
                    }
                    surface
                }
                Err(error) => {
                    eprintln!("flamewm-shell: dock surface create failed: {error}");
                    return;
                }
            },
        };
        let reveal_rect = dock::reveal_rect(placed, output_rect, settings.edge, scale);
        let reveal = match self.dock_stage.reveal {
            Some(reveal) => reveal,
            None => {
                match flamewm_shell::runtime::create_dock_surface(runtime, reveal_rect, scale) {
                    Ok(reveal) => {
                        self.dock_stage.reveal = Some(reveal);
                        reveal
                    }
                    Err(error) => {
                        eprintln!("flamewm-shell: dock reveal create failed: {error}");
                        return;
                    }
                }
            }
        };
        let names: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| {
                if entry.app_id == TRASH_SLOT_ID {
                    return "Trash".to_owned();
                }
                lookup(&entry.app_id).map_or_else(|| entry.app_id.clone(), |app| app.name.clone())
            })
            .collect();
        let state: DockSync = (frame.clone(), placed, names.clone(), layout.entries.clone());
        if self.dock_last.as_ref() != Some(&state) {
            let icon_px = (settings.icon_size * scale).round() as u32;
            let icon_names: Vec<String> = layout
                .entries
                .iter()
                .map(|entry| {
                    if entry.app_id == TRASH_SLOT_ID {
                        return flamewm_shell_core::dock::trash_icon_name(self.dock_trash_full)
                            .to_owned();
                    }
                    lookup(&entry.app_id).map_or_else(String::new, |app| app.icon_name.clone())
                })
                .collect();
            async_projection::bump_dock_view_epoch();
            let icon_loader = &mut self.icon_loader;
            if let Err(error) = runtime.with_document(surface, |document| {
                dock::project_dock(document, &frame)?;
                // Same miss policy as the taskbar and launcher: a cached raster, else the transparent
                // placeholder plus one worker request. The dock's default glyph never shows for a miss.
                for (index, name) in icon_names.iter().enumerate() {
                    let target = IconTarget::DockSlot(index as u8);
                    let icon_id = async_projection::icon_node_id_for_target(target);
                    let mut loader = icon_loader.as_mut();
                    async_projection::schedule_visible(
                        document,
                        &icon_id,
                        &mut loader,
                        target,
                        name,
                        icon_px,
                        icon_px,
                    )?;
                }
                Ok::<(), String>(())
            }) {
                eprintln!("flamewm-shell: dock projection failed: {error:?}");
            }
            let _ = runtime.redraw(surface);
            self.dock_entries.clone_from(&layout.entries);
            self.dock_last = Some(state);
        }
        self.place_dock(
            runtime,
            (surface, reveal),
            &frame,
            DockPlacement {
                placed,
                output: output_rect,
                scale,
                auto_hide: settings.auto_hide,
                shown: !layout.entries.is_empty(),
                model,
            },
        );
        self.sync_dock_label(runtime);
    }

    /// Shows the hover label of the hovered slot as its own popup above or below the bar, or
    /// hides it. The popup follows the dock's slide-adjusted rect and never paints inside the dock.
    pub(crate) fn sync_dock_label(&mut self, runtime: &mut SurfaceRuntime) {
        let dock_mapped = self.dock_stage.mapped.is_some_and(|(dock, _)| dock);
        let hovered = self
            .dock_hover
            .filter(|_| dock_mapped && self.dock_menu_slot.is_none());
        let shown = match (hovered, self.dock_stage.placed, self.dock_last.as_ref()) {
            (Some(index), Some(placed), Some(state)) => {
                let name = state.2.get(index).map_or("", String::as_str);
                state
                    .0
                    .labels
                    .get(index)
                    .copied()
                    .filter(|label| label.w > 0.0 && !name.is_empty())
                    .map(|label| (name.to_owned(), placed, label))
            }
            _ => None,
        };
        let Some((name, placed, label)) = shown else {
            if self.dock_stage.label_shown.take().is_some() {
                if let Some(popup) = self.dock_stage.label {
                    let _ = runtime.hide(popup);
                }
            }
            return;
        };
        let x = placed.0 + label.x.round() as i32;
        let y = placed.1 + label.y.round() as i32;
        // Unchanged text and place: the popup already shows it, so skip the per-tick re-map and
        // repaint that made the label flicker over window content.
        if self.dock_stage.label_shown.as_ref() == Some(&(name.clone(), (x, y))) {
            return;
        }
        let popup = match self.dock_stage.label {
            Some(popup) => popup,
            None => match flamewm_shell::runtime::create_dock_label_surface(
                runtime,
                (x, y, dock::LABEL_W as u32, dock::LABEL_H as u32),
            ) {
                Ok(popup) => {
                    self.dock_stage.label = Some(popup);
                    popup
                }
                Err(error) => {
                    eprintln!("flamewm-shell: dock label create failed: {error}");
                    return;
                }
            },
        };
        // Hidden while it moves and captures: a capture of a mapped label would bake the label itself.
        let _ = runtime.hide(popup);
        let _ = runtime.move_resize(popup, x, y, dock::LABEL_W as u32, dock::LABEL_H as u32);
        let _ = runtime.with_document(popup, |document| {
            document.text("dock-tip-text", name.clone())
        });
        Self::apply_label_backdrop(
            runtime,
            popup,
            (x, y, dock::LABEL_W as u32, dock::LABEL_H as u32),
        );
        let _ = runtime.show(popup);
        let _ = runtime.redraw(popup);
        self.dock_stage.label_shown = Some((name, (x, y)));
    }

    /// Bakes the real pixels beneath the label rect as its backdrop: X11 windows are opaque, so
    /// unbacked white text would sit on a grey box. No tint, so the label shows the true background.
    fn apply_label_backdrop(
        runtime: &mut SurfaceRuntime,
        popup: flamewm_ui_x11::SurfaceHandle,
        rect: (i32, i32, u32, u32),
    ) {
        let words = match runtime.live_backdrop(popup, rect, 0) {
            Ok(Some(flamewm_ui_x11::backdrop::LiveSurfaceFrame::Pixels(words))) => Some(words),
            // The surface already holds these pixels; nothing to upload.
            Ok(Some(flamewm_ui_x11::backdrop::LiveSurfaceFrame::Unchanged)) => return,
            Ok(None) | Err(_) => runtime.desktop_backdrop(rect, 0, 0),
        };
        let _ = runtime
            .set_surface_backdrop(popup, words.as_deref().map(|words| (words, rect.2, rect.3)));
    }

    /// Moves the dock and reveal strip, keeps the work-area strut in step with auto-hide, and maps
    /// exactly one of them: the dock while shown, the reveal strip while hidden.
    fn place_dock(
        &mut self,
        runtime: &mut SurfaceRuntime,
        (surface, reveal): (flamewm_ui_x11::SurfaceHandle, flamewm_ui_x11::SurfaceHandle),
        frame: &dock::DockFrame,
        placement: DockPlacement,
    ) {
        let DockPlacement {
            placed,
            output,
            scale,
            auto_hide,
            shown,
            model,
        } = placement;
        let extent = dock::slide_extent(placed, frame.edge, output);
        let offset = (model.offset() * extent as f32).round() as i32;
        let moved = dock::slide_rect(placed, frame.edge, offset);
        if self.dock_stage.placed != Some(moved) {
            let _ = runtime.move_resize(surface, moved.0, moved.1, moved.2, moved.3);
            self.dock_stage.placed = Some(moved);
        }
        // Only a shown dock samples the desktop under its pill; hidden, it does no capture work.
        if shown && !model.is_hidden() {
            self.apply_dock_backdrop(runtime, surface, moved);
        } else {
            self.dock_stage.backdrop = None;
        }
        let reveal_rect = dock::reveal_rect(placed, output, frame.edge, scale);
        if self.dock_stage.reveal_placed != Some(reveal_rect) {
            let _ = runtime.move_resize(
                reveal,
                reveal_rect.0,
                reveal_rect.1,
                reveal_rect.2,
                reveal_rect.3,
            );
            self.dock_stage.reveal_placed = Some(reveal_rect);
            // The reveal strip never gets a projection, so it needs one paint before it can hit-test.
            let _ = runtime.redraw(reveal);
        }
        // An auto-hidden Dock reserves nothing; a shown one keeps its bar plus the floating margin.
        if self.dock_stage.strut != Some((placed, auto_hide)) {
            let bar_css = if frame.edge.is_vertical() {
                frame.bar.w
            } else {
                frame.bar.h
            };
            let strut =
                (!auto_hide).then(|| ((bar_css + dock::DOCK_MARGIN) * scale).round() as u32);
            // The window is exactly the bar, so a stadium pill clips the window corners with
            // radius = half the bar thickness; without it the unpainted corners show clear pixels.
            let corner_radius = dock::pill_corner_radius(bar_css, scale);
            let _ = runtime.set_dock_frame(
                surface,
                strut.map(|_| strut_edge(frame.edge)),
                corner_radius,
                strut,
            );
            self.dock_stage.strut = Some((placed, auto_hide));
        }
        let hidden = model.is_hidden();
        // The strip stays mapped under a shown auto-hide dock: unmapping it under the pointer
        // sends a Leave that ends the hold and hides the dock again.
        let mapped = (shown && !hidden, shown && (hidden || auto_hide));
        if self.dock_stage.mapped != Some(mapped) {
            let reveal_was_mapped = self.dock_stage.mapped.is_some_and(|(_, reveal)| reveal);
            let _ = if mapped.0 {
                runtime.show(surface)
            } else {
                runtime.hide(surface)
            };
            let _ = if mapped.1 {
                runtime.show(reveal)
            } else {
                runtime.hide(reveal)
            };
            // The strip is painted once at creation while unmapped; repaint on each map so it hit-tests.
            if mapped.1 && !reveal_was_mapped {
                let _ = runtime.redraw(reveal);
            }
            self.dock_stage.mapped = Some(mapped);
        }
    }

    /// Installs the dock's pseudo-transparency: what lies beneath the pill, opacity and blur from
    /// the taskbar material, so changing the surface material changes both together. With windows
    /// beneath (X11 window manager) the live composition shows them; otherwise the desktop layer.
    /// Recomposes on a rect or material change, and on root child changes while the live path is
    /// in use. An opaque material takes no capture.
    fn apply_dock_backdrop(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: flamewm_ui_x11::SurfaceHandle,
        rect: (i32, i32, u32, u32),
    ) {
        let (opacity, blur) = match self.settings_cache.as_ref() {
            Some(settings) => {
                crate::entry::settings_projection::taskbar_backdrop_percents(settings)
            }
            None => return,
        };
        // Opaque material hides whatever lies beneath: drop any backdrop and capture nothing.
        if !flamewm_ui_x11::backdrop::backdrop_needed(opacity, blur) {
            if self.dock_stage.backdrop.take().is_some() {
                let _ = runtime.set_surface_backdrop(surface, None);
            }
            return;
        }
        let key = (rect, opacity, blur);
        let root_changed = runtime.take_root_changed();
        let live_active = runtime.live_backdrop_in_use();
        if !flamewm_ui_x11::backdrop::recompose_needed(
            self.dock_stage.backdrop.as_ref(),
            &key,
            root_changed,
            live_active,
        ) {
            return;
        }
        self.dock_stage.backdrop = Some(key);
        if rect.2 == 0 || rect.3 == 0 {
            let _ = runtime.set_surface_backdrop(surface, None);
            return;
        }
        match runtime.live_backdrop(surface, rect, blur) {
            // The pixels beneath are unchanged: the snapshot already on the dock is current.
            Ok(Some(flamewm_ui_x11::backdrop::LiveSurfaceFrame::Unchanged)) => return,
            Ok(Some(flamewm_ui_x11::backdrop::LiveSurfaceFrame::Pixels(words))) => {
                let _ = runtime.set_surface_backdrop(surface, Some((&words, rect.2, rect.3)));
                return;
            }
            // No redirected frames (no window manager redirect): the desktop layer alone.
            Ok(None) | Err(_) => {}
        }
        let words = runtime.desktop_backdrop(rect, opacity, blur);
        let _ = match words {
            Some(words) => runtime.set_surface_backdrop(surface, Some((&words, rect.2, rect.3))),
            None => runtime.set_surface_backdrop(surface, None),
        };
    }

    /// Pointer over the dock or its reveal strip; keeps the dock shown or starts its hide delay.
    /// Applies the hold at once: the event path does not tick, so this syncs the slide and
    /// republishes the hide deadline itself.
    pub(crate) fn dock_pointer(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: PointerSurface,
        inside: bool,
    ) {
        let now = std::time::Instant::now();
        if let Some(mut model) = self.dock_stage.autohide {
            model.set_pointer(surface, inside, now);
            self.dock_stage.autohide = Some(model);
        }
        self.sync_dock(runtime);
        self.publish_wake(now);
    }

    /// Hover on the reveal strip: the dock slides in while the pointer is over it.
    pub(crate) fn route_dock_reveal(&mut self, runtime: &mut SurfaceRuntime, event: &SurfaceEvent) {
        if event.action.phase == ActionPhase::Hover {
            if event.action.inside {
                // Revealing is the only time the Trash folder is re-read; a change forces a rebuild.
                self.refresh_dock_trash();
            }
            self.dock_pointer(runtime, PointerSurface::Reveal, event.action.inside);
        }
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
    /// Live preview: moves the dragged slot with the pointer (`position` only, no relayout) and
    /// dims it while it sits past the unpin distance. The next rebuild restores its rest place.
    fn preview_dock_drag(&mut self, runtime: &mut SurfaceRuntime, drag: DockDrag) {
        let Some((frame, ..)) = self.dock_last.as_ref() else {
            return;
        };
        let (Some(base), Some(surface)) = (frame.slots.get(drag.from).copied(), self.dock_surface)
        else {
            return;
        };
        let vertical = dock::dock_settings(self.settings_cache.as_ref())
            .edge
            .is_vertical();
        let (origin_x, origin_y) = flamewm_shell_core::dock::drag_preview_origin(
            (base.x, base.y),
            drag.start,
            drag.pos,
            vertical,
        );
        let (cross, bar_extent) = if vertical {
            (drag.pos.0 - frame.bar.x, frame.bar.w)
        } else {
            (drag.pos.1 - frame.bar.y, frame.bar.h)
        };
        let off_bar = drag_off_removes(cross, bar_extent, DRAG_OFF_PX);
        let slot_id = format!("dock-slot-{}", drag.from + 1);
        let result = runtime.with_document(surface, |document| {
            document.position(&slot_id, origin_x, origin_y)?;
            document.opacity(&slot_id, if off_bar { 0.4 } else { 1.0 })
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: dock drag preview failed: {error:?}");
        }
    }

    fn finish_dock_drag(&mut self, drag: DockDrag) {
        // Taking the cached state forces the next sync to rebuild on every exit below, which
        // restores the preview's slot place and opacity.
        let Some(state) = self.dock_last.take() else {
            return;
        };
        let frame = state.0;
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
        if event.action.phase == ActionPhase::Hover {
            self.dock_pointer(runtime, PointerSurface::Dock, event.action.inside);
        }
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
                if let Some(drag) = self.dock_drag.filter(|drag| drag.active) {
                    self.preview_dock_drag(runtime, drag);
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
            // Hit-test the un-magnified slot rects, not the document node under the pointer: a
            // hover-grown icon can extend past its slot, and a hit on that overhang would
            // un-hover the slot, shrink the icon, and re-hover it on the next event.
            let hovered = self
                .dock_last
                .as_ref()
                .and_then(|state| {
                    let rects: Vec<(f32, f32, f32, f32)> =
                        state.0.slots.iter().map(|r| (r.x, r.y, r.w, r.h)).collect();
                    flamewm_shell_core::dock::hover_slot(x, y, &rects)
                })
                .filter(|_| event.action.inside && self.dock_menu_slot.is_none())
                .filter(|index| *index < self.dock_entries.len())
                // A slot with no name would paint an empty label box above the bar.
                .filter(|index| {
                    self.dock_last
                        .as_ref()
                        .and_then(|state| state.2.get(*index))
                        .is_some_and(|name| !name.is_empty())
                });
            if hovered != self.dock_hover {
                self.set_dock_hover_scale(runtime, hovered);
                self.dock_hover = hovered;
                self.sync_dock_label(runtime);
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
        if entry.app_id == TRASH_SLOT_ID {
            if let Err(error) = flamewm_applications::open_directory(&dock::trash_files_dir()) {
                eprintln!("flamewm-shell: trash open failed: {error}");
            }
            return Ok(());
        }
        let request = {
            let snapshot = self.shell.snapshot();
            let windows = &snapshot.windows;
            // A launch (no running window yet) starts the optional hop; a focus click does not.
            if !entry.running && dock::dock_settings(self.settings_cache.as_ref()).launch_bounce {
                self.dock_bounce = Some((
                    slot.wrapping_sub(1),
                    flamewm_shell_core::dock::LaunchBounce::start(
                        std::time::Instant::now(),
                        windows.len(),
                    ),
                ));
            }
            dock::click_request(&entry.app_id, windows)
        };
        self.submit(request);
        Ok(())
    }
}

impl ShellLoop {
    /// Subtle hover scale (`dock.hoverScale`): grows the hovered icon and returns the previous one
    /// to rest. Only two icon nodes are written, so the repaint stays inside the dock.
    fn set_dock_hover_scale(&mut self, runtime: &mut SurfaceRuntime, hovered: Option<usize>) {
        if !dock::dock_settings(self.settings_cache.as_ref()).hover_scale {
            return;
        }
        let (Some((frame, ..)), Some(surface)) = (self.dock_last.as_ref(), self.dock_surface)
        else {
            return;
        };
        let mut writes = Vec::new();
        for (index, scaled) in [(self.dock_hover, false), (hovered, true)] {
            let Some(i) = index else { continue };
            let (Some(slot), Some(icon)) = (frame.slots.get(i), frame.icons.get(i)) else {
                continue;
            };
            let base = (icon.x - slot.x, icon.y - slot.y, icon.w, icon.h);
            let rect = if scaled {
                hover_scaled_rect(base, HOVER_SCALE, frame.edge)
            } else {
                base
            };
            writes.push((format!("dock-slot-{}-icon", i + 1), rect));
        }
        if writes.is_empty() {
            return;
        }
        let result = runtime.with_document(surface, |document| {
            for (id, (x, y, w, h)) in &writes {
                document.position(id, *x, *y)?;
                document.size(id, *w, *h)?;
            }
            Ok::<(), String>(())
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: dock hover scale failed: {error:?}");
        }
    }

    /// Advances the launch hop: writes the icon's lifted position while it runs, then the rest
    /// position once. Nothing is written while the dock is hidden or rebuilding.
    pub(crate) fn tick_dock_bounce(
        &mut self,
        runtime: &mut SurfaceRuntime,
        now: std::time::Instant,
    ) {
        let Some((index, bounce)) = self.dock_bounce else {
            return;
        };
        let windows = self.shell.snapshot().windows.len();
        let lift = bounce.lift(now).filter(|_| !bounce.arrived(windows));
        if lift.is_none() {
            // The first window (or the period's end) settles the icon back to rest.
            self.dock_bounce = None;
        }
        let hidden = self
            .dock_stage
            .autohide
            .as_ref()
            .is_some_and(AutoHide::is_hidden);
        let (Some((frame, ..)), Some(surface), false) =
            (self.dock_last.as_ref(), self.dock_surface, hidden)
        else {
            return;
        };
        let (Some(slot), Some(icon)) = (frame.slots.get(index), frame.icons.get(index)) else {
            return;
        };
        let (x, y) = (icon.x - slot.x, icon.y - slot.y);
        let lifted = lift.map_or(0.0_f32, |lift| lift as f32);
        let id = format!("dock-slot-{}-icon", index + 1);
        let result =
            runtime.with_document(surface, |document| document.position(&id, x, y - lifted));
        if let Err(error) = result {
            eprintln!("flamewm-shell: dock launch bounce failed: {error:?}");
        }
    }

    /// Re-reads the Trash folder. A changed state drops the cached dock so the next sync rebuilds the icon.
    pub(crate) fn refresh_dock_trash(&mut self) {
        let full = flamewm_shell_core::dock::trash_has_items(&dock::trash_data_home());
        if full != self.dock_trash_full {
            self.dock_trash_full = full;
            self.dock_last = None;
        }
    }

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
        if entry.app_id == TRASH_SLOT_ID {
            // The Trash slot has no pin or run state to act on.
            return;
        }
        let Some(slot) = frame.slots.get(index).copied() else {
            return;
        };
        let items = flamewm_shell_core::dock::menu_items(entry.pinned, entry.running);
        let scale = rect.2 as f32 / frame.width.max(1.0);
        let (w, h) = (180.0 * scale, (items.len() as f32 * 28.0 + 10.0) * scale);
        let (w, h) = (w.round() as i32, h.round() as i32);
        let bar = frame.bar;
        let (x, y) = match frame.edge {
            Edge::Bottom => (
                rect.0 + (slot.x * scale) as i32,
                rect.1 + ((bar.y * scale) as i32) - h - 4,
            ),
            Edge::Top => (
                rect.0 + (slot.x * scale) as i32,
                rect.1 + (((bar.y + bar.h) * scale) as i32) + 4,
            ),
            Edge::Left => (
                rect.0 + (((bar.x + bar.w) * scale) as i32) + 4,
                rect.1 + (slot.y * scale) as i32,
            ),
            Edge::Right => (
                rect.0 + ((bar.x * scale) as i32) - w - 4,
                rect.1 + (slot.y * scale) as i32,
            ),
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
            self.set_dock_hover_scale(runtime, None);
            self.dock_hover = None;
            let _ = runtime.redraw(dock_surface);
            self.sync_dock_label(runtime);
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
                if !ids.contains(&entry.app_id) && ids.len() < flamewm_shell_core::dock::MAX_PINNED
                {
                    ids.push(entry.app_id.clone());
                    self.persist_dock_pins(ids);
                }
            }
            flamewm_shell_core::dock::DockMenuAction::RemoveFromDock => {
                let ids: Vec<String> = pinned_ids
                    .into_iter()
                    .filter(|id| *id != entry.app_id)
                    .collect();
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
                    .map(|w| w.reference)
                    .collect();
                for window in windows {
                    self.submit(ControlRequest::CloseWindow(window));
                }
            }
        }
        Ok(())
    }
}
