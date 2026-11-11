//! Lock surface lifecycle: show, raise, rebuild per output, root geometry and teardown.

use super::*;

impl LockLoop {
    pub(super) fn show_surfaces(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if let Some(fallback) = self.fallback.as_ref() {
            show_raised(runtime, fallback.handle, "lock coverage")?;
        }
        for (index, surface) in self.surfaces.iter().enumerate() {
            show_raised(
                runtime,
                surface.handle,
                &format!("lock monitor {}", index + 1),
            )?;
        }
        Ok(())
    }

    pub(super) fn raise_surfaces(&self, runtime: &SurfaceRuntime) -> Result<(), String> {
        if let Some(fallback) = self.fallback.as_ref() {
            runtime
                .raise(fallback.handle)
                .map_err(|error| format!("raise lock coverage: {error:?}"))?;
        }
        for surface in &self.surfaces {
            runtime
                .raise(surface.handle)
                .map_err(|error| format!("raise lock surface: {error:?}"))?;
        }
        Ok(())
    }

    pub(super) fn apply_display_snapshot(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &DisplaySnapshot,
    ) {
        let outputs = crate::session::lock_output_rects_from_snapshot(snapshot, self.root);
        let current = self
            .surfaces
            .iter()
            .map(|surface| surface.rect)
            .collect::<Vec<_>>();
        if output_rects_need_rebuild(&current, &outputs)
            && !self.rebuild_output_surfaces(runtime, outputs)
        {
            self.pending_display_snapshot = Some(snapshot.clone());
        }
    }

    pub(super) fn rebuild_output_surfaces(
        &mut self,
        runtime: &mut SurfaceRuntime,
        rects: Vec<Rect>,
    ) -> bool {
        let mut rebuilt = Vec::with_capacity(rects.len());
        let canvas = document::shared_root_background(&self.settings, self.root);
        for (index, rect) in rects.iter().enumerate() {
            match create_lock_surface(
                runtime,
                &self.settings,
                self.root,
                canvas.as_ref(),
                *rect,
                &format!("FlameWM lock {}", index + 1),
            ) {
                Ok(surface) => rebuilt.push(surface),
                Err(error) => {
                    for surface in &rebuilt {
                        let _ = runtime.destroy(surface.handle);
                    }
                    eprintln!("flamewm-lock: monitor surface rebuild failed: {error}");
                    return false;
                }
            }
        }
        for (index, surface) in rebuilt.iter().enumerate() {
            if let Err(error) = show_raised(
                runtime,
                surface.handle,
                &format!("lock monitor {}", index + 1),
            ) {
                for surface in &rebuilt {
                    let _ = runtime.destroy(surface.handle);
                }
                eprintln!("flamewm-lock: monitor surface show failed: {error}");
                return false;
            }
        }
        let old = std::mem::replace(&mut self.surfaces, rebuilt);
        for surface in old {
            let _ = runtime.destroy(surface.handle);
        }
        self.frames = None;
        self.fallback_frames = None;
        self.shown = None;
        self.shown_power = None;
        self.ui_visibility_dirty = true;
        self.repaint(runtime);
        keep_lock_step("raise refreshed lock surfaces", || {
            self.raise_surfaces(runtime)
        });
        true
    }

    pub(super) fn refresh_root_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        width: u32,
        height: u32,
    ) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let root = Rect::new(0, 0, width as i32, height as i32);
        if root == self.root {
            return true;
        }
        let Some(fallback) = self.fallback.as_mut() else {
            return false;
        };
        let canvas = document::shared_root_background(&self.settings, root);
        let background =
            document::background_for_output(&self.settings, root, root, canvas.as_ref());
        if let Err(error) = runtime.move_resize(fallback.handle, root.x, root.y, width, height) {
            eprintln!("flamewm-lock: resize root coverage failed; retaining prior lock: {error:?}");
            return false;
        }
        if let Err(error) = runtime.raise(fallback.handle) {
            eprintln!("flamewm-lock: raise resized root coverage failed: {error:?}");
            return false;
        }
        if let Err(error) = runtime.with_document(fallback.handle, |view| {
            document::apply_document(view, &self.settings, width, height, background.clone())
        }) {
            eprintln!("flamewm-lock: theme resized root coverage failed: {error:?}");
            return false;
        }
        fallback.rect = root;
        fallback.base_background = background;
        self.root = root;
        self.shown = None;
        self.shown_power = None;
        self.ui_visibility_dirty = true;
        self.repaint(runtime);

        // The full-root lock was resized first, so failures during monitor reprojection
        // keep the desktop covered by the same authentication session.
        let rects = crate::session::lock_output_rects(root);
        if !self.rebuild_output_surfaces(runtime, rects) {
            eprintln!(
                "flamewm-lock: root resized; prior monitor surfaces retained under root lock"
            );
        }
        true
    }

    pub(super) fn destroy_surfaces(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for surface in &self.surfaces {
            runtime
                .destroy(surface.handle)
                .map_err(|error| format!("destroy lock surface: {error:?}"))?;
        }
        if let Some(fallback) = self.fallback.as_ref() {
            runtime
                .destroy(fallback.handle)
                .map_err(|error| format!("destroy lock coverage surface: {error:?}"))?;
        }
        Ok(())
    }
}
