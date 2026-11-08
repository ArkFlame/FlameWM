//! Tray tick: opens a pending dbusmenu popup and re-projects the panel item slots after a
//! tray change.

use super::*;

impl ShellLoop {
    /// Reactor tick for the tray. The D-Bus source only records changes; the panel writes and
    /// the popup open happen here.
    pub(crate) fn tick_tray(&mut self, runtime: &mut SurfaceRuntime) {
        let Some(tray) = self.tray.clone() else {
            return;
        };
        let opened = tray.borrow_mut().take_menu_ready();
        if let Some(menu) = opened {
            self.shell
                .open_tray_menu(menu.key, menu.menu_path, menu.rows, menu.pointer);
            if let Err(error) = self.apply_controls(runtime) {
                eprintln!("flamewm-shell: tray menu open failed: {error:?}");
            }
        }
        if !tray.borrow_mut().take_dirty() {
            return;
        }
        self.project_tray_panels(runtime);
    }

    /// Projects the tray items into every panel. Called on tray change and after a panel
    /// surface is created or resynced, so a new panel shows the items already in the tray.
    /// Returns `false` when a panel refused the projection; the host is then marked dirty.
    pub(crate) fn project_tray_panels(&mut self, runtime: &mut SurfaceRuntime) -> bool {
        let Some(tray) = self.tray.clone() else {
            return true;
        };
        self.ensure_loader();
        let host = tray.borrow();
        let loader = &mut self.icon_loader;
        for panel in self.surfaces.panel_handles() {
            let projected =
                runtime.with_document(panel, |document| host.project(document, loader.as_mut()));
            let redrawn = projected.is_ok() && runtime.redraw(panel).is_ok();
            if let Err(error) = &projected {
                eprintln!("flamewm-shell: tray projection failed: {error:?}");
            }
            if !redrawn {
                drop(host);
                tray.borrow_mut().mark_dirty();
                return false;
            }
        }
        true
    }
}
