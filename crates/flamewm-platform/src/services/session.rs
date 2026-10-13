use flamewm_api::ports::SessionPort;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionService;

impl SessionService {
    #[must_use]
    pub fn capabilities<P: SessionPort>(&self, port: &P) -> SessionCapabilities {
        port.session_capabilities()
    }

    pub fn perform<P: SessionPort>(&self, port: &mut P, action: SessionAction) -> FlameResult<()> {
        let caps = port.session_capabilities();
        let available = match action {
            SessionAction::Lock => caps.lock,
            SessionAction::LockPreview => caps.lock,
            SessionAction::Logout => caps.logout,
            SessionAction::Suspend => caps.suspend,
            SessionAction::Reboot => caps.reboot,
            SessionAction::Shutdown => caps.shutdown,
        };
        if !available {
            return Err(FlameError::new(
                ErrorCode::Unavailable,
                "session action is unavailable",
            ));
        }
        port.perform_session_action(action)
    }
}
