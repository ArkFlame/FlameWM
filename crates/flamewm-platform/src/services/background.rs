use flamewm_api::background::BackgroundState;
use flamewm_api::ports::BackgroundPort;
use flamewm_api::{ErrorCode, FlameError, FlameResult};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundService {
    current: Option<BackgroundState>,
    revision: u64,
}

impl BackgroundService {
    #[must_use]
    pub fn current(&self) -> Option<&BackgroundState> {
        self.current.as_ref()
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply<P: BackgroundPort>(
        &mut self,
        port: &mut P,
        state: BackgroundState,
    ) -> FlameResult<()> {
        if state.wallpaper_path.trim().is_empty() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "wallpaper path is empty",
            ));
        }
        if self.current.as_ref() == Some(&state) {
            return Ok(());
        }
        port.project_background(&state)?;
        port.reload_background()?;
        self.current = Some(state);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }
}
