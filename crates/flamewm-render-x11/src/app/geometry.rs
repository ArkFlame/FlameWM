//! Transactional geometry commit + presenter helpers on X11App.

use super::*;
use crate::WindowGeometry;

impl X11App {
    /// Transactional geometry commit: retained size -> backbuffer ->
    /// retarget Xft/XRender -> resize window -> shape -> repaint -> flush.
    /// Any failure aborts before partial state; the error is never swallowed.
    pub(crate) unsafe fn commit_geometry(
        &mut self,
        document: &RuntimeDocument,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let commit = GeometryCommit::new(width, height).map_err(|e| presenter_error(&e))?;
        if commit.width == self.width && commit.height == self.height {
            return Ok(());
        }
        let (prev_w, prev_h) = (self.width, self.height);
        self.width = commit.width;
        self.height = commit.height;
        if let Err(error) = unsafe { self.recreate_backbuffer() } {
            self.width = prev_w;
            self.height = prev_h;
            return Err(presenter_error(&error));
        }
        unsafe { XResizeWindow(self.display, self.window, commit.width, commit.height) };
        if let Err(error) = unsafe { self.redraw(document) } {
            return Err(presenter_error(&error));
        }
        unsafe { self.refresh_shape_mask(document) };
        unsafe { XFlush(self.display) };
        // Geometry commit is only legal from Projected; later states keep
        // their stage (commit is idempotent, not a backward transition).
        if self.presentation == PresentationState::Projected {
            self.presentation = self
                .presentation
                .advance_to(PresentationState::GeometryCommitted)?;
        }
        Ok(())
    }

    /// Apply a requested move+size to the app window, then run the same
    /// transactional commit as any other geometry change. `None` is a no-op.
    pub(crate) unsafe fn apply_window_geometry(
        &mut self,
        document: &RuntimeDocument,
        request: Option<WindowGeometry>,
    ) -> Result<(), String> {
        let Some(request) = request else {
            return Ok(());
        };
        if request.width == 0 || request.height == 0 {
            return Err(presenter_error("window geometry request must be non-empty"));
        }
        unsafe {
            crate::surface_controller::SurfaceController::move_app_window_geometry(
                self, document, request,
            )
        }
    }

    /// First present per SurfaceAlphaMode: composited ARGB scene is already
    /// transparent-cleared; opaque fallback paints flattened pixels.
    /// Then advance CreatedHidden/Projected toward Painted.
    #[allow(dead_code)]
    pub(crate) unsafe fn present_first(
        &mut self,
        document: &RuntimeDocument,
    ) -> Result<(), String> {
        unsafe { self.redraw(document).map_err(|e| presenter_error(&e)) }?;
        unsafe { XFlush(self.display) };
        // Walk forward legally: CreatedHidden -> Projected -> GeometryCommitted -> Painted.
        for next in [
            PresentationState::Projected,
            PresentationState::GeometryCommitted,
            PresentationState::Painted,
        ] {
            if self.presentation == next {
                continue;
            }
            match self.presentation.advance_to(next) {
                Ok(state) => self.presentation = state,
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Chrome-ready path: repaint via the safe external drawable painter
    /// (same semantics: transparent clear, canonical scene target, present
    /// per SurfaceAlphaMode). Errors propagate, never swallowed.
    #[allow(dead_code)]
    pub(crate) unsafe fn chrome_ready_repaint(
        &mut self,
        document: &RuntimeDocument,
    ) -> Result<(), String> {
        unsafe { self.redraw(document) }?;
        let mut painter = crate::NativeDrawableRenderer::new(self);
        painter.flush();
        Ok(())
    }

    /// Record MapNotify arrival: Painted -> Mapped; error otherwise.
    pub(crate) fn note_mapped(&mut self) -> Result<(), String> {
        self.presentation = self.presentation.note_mapped()?;
        Ok(())
    }

    /// Advance Mapped -> Presented after the mapped paint is flushed.
    #[allow(dead_code)]
    pub(crate) unsafe fn present_mapped(
        &mut self,
        document: &RuntimeDocument,
    ) -> Result<(), String> {
        if self.presentation != PresentationState::Mapped {
            return Err(presenter_error("present_mapped requires Mapped state"));
        }
        unsafe { self.redraw(document).map_err(|e| presenter_error(&e)) }?;
        unsafe { XFlush(self.display) };
        self.presentation = self.presentation.advance_to(PresentationState::Presented)?;
        Ok(())
    }
}
