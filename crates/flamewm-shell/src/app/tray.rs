//! Tray projection tick: re-projects the panel item slots after a tray change.

use super::*;

impl ShellLoop {
    /// Projects tray items into every panel when the tray host reports a change. The D-Bus
    /// source only records the change; the reactor tick does the panel writes.
    pub(crate) fn tick_tray(&mut self, runtime: &mut SurfaceRuntime) {
        let Some(tray) = self.tray.clone() else {
            return;
        };
        if !tray.borrow_mut().take_dirty() {
            return;
        }
        self.ensure_loader();
        let host = tray.borrow();
        let loader = &mut self.icon_loader;
        for panel in self.surfaces.panel_handles() {
            let projected = runtime.with_document(panel, |document| {
                host.project(document, loader.as_mut())
            });
            let redrawn = projected.is_ok() && runtime.redraw(panel).is_ok();
            if let Err(error) = &projected {
                eprintln!("flamewm-shell: tray projection failed: {error:?}");
            }
            if !redrawn {
                drop(host);
                tray.borrow_mut().mark_dirty();
                return;
            }
        }
    }
}
