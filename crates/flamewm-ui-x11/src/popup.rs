use crate::{SurfaceHandle, SurfaceRuntime, UiBackendError};

#[derive(Debug, Default)]
pub struct PopupState {
    open: Option<SurfaceHandle>,
}

impl PopupState {
    pub fn open(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
    ) -> Result<(), UiBackendError> {
        if self.open == Some(surface) {
            return Ok(());
        }
        if let Some(previous) = self.open.take() {
            runtime.hide(previous)?;
        }
        runtime.show(surface)?;
        self.open = Some(surface);
        Ok(())
    }

    pub fn close(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        let Some(surface) = self.open.take() else { return Ok(()); };
        runtime.hide(surface)
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn active(&self) -> Option<SurfaceHandle> {
        self.open
    }
}
