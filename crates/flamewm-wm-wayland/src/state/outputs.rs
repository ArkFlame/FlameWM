use super::*;

impl State {
    pub fn output_rect(&self) -> Rect {
        Rect::new(0, 0, self.output_size.w, self.output_size.h)
    }

    /// Name of the output a surface sits on, or the primary output when it is not mapped.
    pub fn output_name_of(&self, surface: &WlSurface) -> String {
        let key = smithay::reexports::wayland_server::Resource::id(surface);
        self.surface_outputs
            .output_of(&key)
            .map_or_else(|| self.primary_name.clone(), str::to_owned)
    }

    /// Output handle for a connector name; the primary output when the name is not driven.
    ///
    /// The cached primary name answers the common case without cloning every output's name just to
    /// compare it: `Output::name` allocates a `String` from its lock on each call.
    pub fn output_handle(&self, name: &str) -> Output {
        if name == self.primary_name {
            return self.output.clone();
        }
        self.output_handles
            .iter()
            .find(|out| out.name() == name)
            .unwrap_or(&self.output)
            .clone()
    }

    /// Output handle a surface sits on, the primary output when unmapped.
    pub fn output_handle_of(&self, surface: &WlSurface) -> Output {
        let key = smithay::reexports::wayland_server::Resource::id(surface);
        // Every commit resolves this. The two lookups stay reference-only: the home name is compared
        // against the cached primary name first (no allocation, and it is where most surfaces live),
        // and only a real secondary output pays for the handle scan.
        let Some(name) = self.surface_outputs.output_of(&key) else {
            return self.output.clone();
        };
        if name == self.primary_name {
            return self.output.clone();
        }
        self.output_handles
            .iter()
            .find(|out| out.name() == name)
            .unwrap_or(&self.output)
            .clone()
    }

    /// Records the output a surface sits on. Sends `wl_surface.leave` on the output it left and
    /// `wl_surface.enter` on the new one; smithay tracks each output's surfaces, so repeats are no-ops.
    pub fn set_surface_output(&mut self, surface: &WlSurface, name: String) {
        let previous = self.output_handle_of(surface);
        if crate::output_layout::changes_output(&previous.name(), &name) {
            previous.leave(surface);
        }
        self.output_handle(&name).enter(surface);
        let key = smithay::reexports::wayland_server::Resource::id(surface);
        self.surface_outputs.assign(key, name);
    }

    /// Drops a surface's output entry, leaving the output it was on.
    pub fn forget_surface_output(&mut self, surface: &WlSurface) {
        self.output_handle_of(surface).leave(surface);
        let key = smithay::reexports::wayland_server::Resource::id(surface);
        self.surface_outputs.remove(&key);
    }

    /// Drops output entries for surfaces whose client is gone.
    ///
    /// Precedent: `.vendor/smithay-master/src/desktop/space/mod.rs:427` prunes dead elements on
    /// `refresh`, and `.vendor/cosmic-epoch/cosmic-comp/src/shell/mod.rs:1675` and `:2763` keep
    /// only live surfaces in long-lived collections. A destroyed `wl_surface` can no longer be
    /// resolved by id, so one sweep per paint bounds this map to live surfaces.
    pub fn prune_dead_surface_outputs(&mut self) {
        use smithay::reexports::wayland_server::Resource as _;
        let display = self.display.clone();
        self.surface_outputs.retain_keys(|key| {
            WlSurface::from_id(&display, key.clone()).is_ok_and(|s| s.is_alive())
        });
    }

    /// Leaves every surface on a removed output and moves its layer surfaces to the primary output's
    /// layer map, so their panels keep their exclusive zone on the output that is still present.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))] // called only by the drm output sync
    pub(crate) fn retire_output(&mut self, out: &Output) {
        let name = out.name();
        let homes: Vec<(WlSurface, String)> = self
            .toplevels
            .iter()
            .map(|(_, top)| {
                (
                    top.wl_surface().clone(),
                    self.output_name_of(top.wl_surface()),
                )
            })
            .collect();
        for wl in crate::output_layout::homed_on(&homes, &name) {
            out.leave(&wl);
        }
        // Popups are dismissed before their roots move: a popup hangs off a window or a layer surface
        // (nested popups included, via PopupManager's tree walk), and is dismissed when either it or its
        // root is still tagged with the removed output. A popup of a window that moved earlier keeps a
        // stale tag, so checking the popup's own tag as well closes that leak. Choice: dismiss with
        // `send_popup_done`, the primitive smithay uses to close a popup (`.vendor/smithay-master/src/
        // desktop/wayland/popup/mod.rs:78`); KWin is not vendored, so no KWin precedent is cited.
        let layers: Vec<smithay::desktop::LayerSurface> =
            layer_map_for_output(out).layers().cloned().collect();
        let roots: Vec<WlSurface> = self
            .toplevels
            .iter()
            .map(|(_, top)| top.wl_surface().clone())
            .chain(layers.iter().map(|layer| layer.wl_surface().clone()))
            .collect();
        let mut popups = Vec::new();
        for root in &roots {
            let root_output = self.output_name_of(root);
            for (popup, _) in PopupManager::popups_for_surface(root) {
                if let PopupKind::Xdg(xdg) = popup {
                    let popup_output = self.output_name_of(xdg.wl_surface());
                    popups.push((xdg, popup_output, root_output.clone()));
                }
            }
        }
        for xdg in crate::output_layout::popups_to_dismiss(popups, &name) {
            xdg.send_popup_done();
            out.leave(xdg.wl_surface());
            let key = smithay::reexports::wayland_server::Resource::id(xdg.wl_surface());
            self.surface_outputs.remove(&key);
        }
        // Windows homed on the removed output move onto the primary work area, keeping their offset
        // from the removed output.
        let removed = self.output_rect_on(&name);
        let work = self.work_area();
        let moved: Vec<_> = self
            .toplevels
            .iter()
            .filter(|(_, top)| self.output_name_of(top.wl_surface()) == name)
            .map(|(id, top)| (*id, top.wl_surface().clone()))
            .collect();
        for (id, _) in &moved {
            if let Some(rect) = self.model.get(*id).map(|w| w.rect) {
                self.model
                    .set_rect(*id, crate::output_layout::migrate_rect(rect, removed, work));
            }
            self.apply_geometry(*id);
        }
        let primary = self.output.clone();
        let primary_name = self.primary_name.clone();
        for layer in layers {
            layer_map_for_output(out).unmap_layer(&layer);
            let _ = layer_map_for_output(&primary).map_layer(&layer);
            self.set_surface_output(layer.wl_surface(), primary_name.clone());
        }
    }

    /// Homes every toplevel on the output holding most of its rect, then re-sends the scale to the
    /// surfaces whose output scale changed. With no placed row (nested) every window stays primary.
    pub fn refresh_window_outputs(&mut self) {
        let before = crate::presentation_scale::scale_snapshot(self);
        let homes: Vec<(WlSurface, String)> = self
            .toplevels
            .iter()
            .filter_map(|(id, top)| {
                let rect = self.model.get(*id)?.rect;
                let name = crate::output_layout::output_name_for_rect(
                    &self.pointer_layout,
                    &self.layout_outputs,
                    rect,
                    &self.primary_name,
                );
                Some((top.wl_surface().clone(), name.to_owned()))
            })
            .collect();
        for (surface, name) in homes {
            self.set_surface_output(&surface, name);
        }
        crate::presentation_scale::notify_scale_change(self, &before);
    }

    /// Scale of the output a surface sits on, or the primary output's scale when unmapped.
    pub fn surface_scale(&self, surface: &WlSurface) -> f64 {
        let key = smithay::reexports::wayland_server::Resource::id(surface);
        crate::output_layout::scale_for_output(
            &self.output_scales,
            self.surface_outputs.output_of(&key),
            self.output_scale(),
        )
    }

    /// Name of the output under the pointer, or the primary output when the row is unknown.
    pub fn output_name_under_pointer(&self) -> String {
        crate::output_layout::output_at(&self.pointer_layout, self.pointer.x, self.pointer.y)
            .and_then(|index| self.layout_outputs.get(index).cloned())
            .unwrap_or_else(|| self.primary_name.clone())
    }

    /// Output scale factor (1.0 by default). Layout is logical; rendering multiplies by this.
    pub fn output_scale(&self) -> f64 {
        self.output.current_scale().fractional_scale()
    }

    /// Physical pixels of the scanned-out mode, the size desktop captures and offscreen targets use.
    pub fn physical_output_size(&self) -> Size<i32, Physical> {
        self.output.current_mode().map_or_else(
            || (self.output_size.w, self.output_size.h).into(),
            |mode| mode.size,
        )
    }
}
