//! Power buttons on the lock screen: a capability-gated reducer plus the
//! control-plane port that performs the session actions.

use std::time::Duration;

use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(2);
const PERFORM_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a failure message stays visible.
const ERROR_TTL: Duration = Duration::from_secs(4);

/// Performs session actions. Implementations must not panic.
pub trait PowerPort {
    fn capabilities(&self) -> SessionCapabilities;
    fn perform(&mut self, action: SessionAction) -> Result<(), String>;
}

/// [`PowerPort`] backed by the control client. Capabilities are fetched once;
/// a missing bus or failed fetch yields all-false capabilities (fail closed).
pub struct ControlPower {
    client: Option<ControlClient>,
    capabilities: SessionCapabilities,
}

impl ControlPower {
    pub fn connect(kind: BusKind) -> Self {
        Self::from_connect(|| ControlClient::connect(kind))
    }

    /// Builds the port from a fallible connect; a failed connect fails closed.
    pub fn from_connect<E>(connect: impl FnOnce() -> Result<ControlClient, E>) -> Self {
        let client = connect().ok();
        let capabilities = client
            .as_ref()
            .and_then(|client| {
                match client
                    .call_with_timeout(&ControlRequest::GetSessionCapabilities, CAPABILITIES_TIMEOUT)
                    .ok()?
                {
                    ControlResponse::SessionCapabilities(caps) => Some(caps),
                    _ => None,
                }
            })
            .unwrap_or_default();
        Self {
            client,
            capabilities,
        }
    }
}

impl PowerPort for ControlPower {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn perform(&mut self, action: SessionAction) -> Result<(), String> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "control bus unavailable".to_string())?;
        match client.call_with_timeout(&ControlRequest::SessionAction(action), PERFORM_TIMEOUT) {
            Ok(ControlResponse::Unit) => Ok(()),
            Ok(_) => Err("unexpected control reply".to_string()),
            Err(_) => Err("control call failed".to_string()),
        }
    }
}

/// Power button on the lock screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerButton {
    Sleep,
    Restart,
    Shutdown,
    Other,
}

impl PowerButton {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sleep => "Sleep",
            Self::Restart => "Restart",
            Self::Shutdown => "Shut Down",
            Self::Other => "Other…",
        }
    }

    /// `Other` is enabled only when it has an entry to show (log out).
    pub fn enabled(self, caps: SessionCapabilities) -> bool {
        match self {
            Self::Sleep => caps.suspend,
            Self::Restart => caps.reboot,
            Self::Shutdown => caps.shutdown,
            Self::Other => caps.logout,
        }
    }
}

/// Actions that need a confirmation click first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmable {
    Restart,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerState {
    Idle,
    Confirm(Confirmable),
    OtherOpen,
    Busy,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerEvent {
    Click(PowerButton),
    Confirm,
    Cancel,
    Escape,
    ClickLogout,
    Tick(Duration),
    Result(Result<(), String>),
}

/// Side effect requested by the reducer. There is deliberately no variant that unlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Perform(SessionAction),
}

/// Pure reducer for the power buttons.
#[derive(Debug, Clone)]
pub struct PowerUi {
    capabilities: SessionCapabilities,
    state: PowerState,
    error_age: Duration,
}

impl PowerUi {
    pub fn new(capabilities: SessionCapabilities) -> Self {
        Self {
            capabilities,
            state: PowerState::Idle,
            error_age: Duration::ZERO,
        }
    }

    pub fn state(&self) -> &PowerState {
        &self.state
    }

    pub fn handle(&mut self, event: PowerEvent) -> Effect {
        match event {
            PowerEvent::Click(button) => self.click(button),
            PowerEvent::Confirm => self.confirm(),
            PowerEvent::Cancel | PowerEvent::Escape => {
                if matches!(
                    self.state,
                    PowerState::Confirm(_) | PowerState::OtherOpen | PowerState::Error(_)
                ) {
                    self.set(PowerState::Idle);
                }
                Effect::None
            }
            PowerEvent::ClickLogout => {
                if self.state == PowerState::OtherOpen && self.capabilities.logout {
                    self.set(PowerState::Busy);
                    Effect::Perform(SessionAction::Logout)
                } else {
                    Effect::None
                }
            }
            PowerEvent::Tick(elapsed) => {
                if matches!(self.state, PowerState::Error(_)) {
                    self.error_age += elapsed;
                    if self.error_age >= ERROR_TTL {
                        self.set(PowerState::Idle);
                    }
                }
                Effect::None
            }
            PowerEvent::Result(outcome) => {
                if self.state == PowerState::Busy {
                    self.set(match outcome {
                        Ok(()) => PowerState::Idle,
                        Err(message) => PowerState::Error(message),
                    });
                }
                Effect::None
            }
        }
    }

    fn click(&mut self, button: PowerButton) -> Effect {
        if !matches!(self.state, PowerState::Idle | PowerState::Error(_))
            || !button.enabled(self.capabilities)
        {
            return Effect::None;
        }
        match button {
            PowerButton::Sleep => {
                self.set(PowerState::Busy);
                Effect::Perform(SessionAction::Suspend)
            }
            PowerButton::Restart => {
                self.set(PowerState::Confirm(Confirmable::Restart));
                Effect::None
            }
            PowerButton::Shutdown => {
                self.set(PowerState::Confirm(Confirmable::Shutdown));
                Effect::None
            }
            PowerButton::Other => {
                self.set(PowerState::OtherOpen);
                Effect::None
            }
        }
    }

    fn confirm(&mut self) -> Effect {
        let PowerState::Confirm(action) = self.state else {
            return Effect::None;
        };
        let (action, enabled) = match action {
            Confirmable::Restart => (SessionAction::Reboot, self.capabilities.reboot),
            Confirmable::Shutdown => (SessionAction::Shutdown, self.capabilities.shutdown),
        };
        if enabled {
            self.set(PowerState::Busy);
            Effect::Perform(action)
        } else {
            self.set(PowerState::Idle);
            Effect::None
        }
    }

    fn set(&mut self, state: PowerState) {
        self.state = state;
        self.error_age = Duration::ZERO;
    }
}

#[cfg(test)]
pub struct FakePower {
    pub capabilities: SessionCapabilities,
    pub calls: Vec<SessionAction>,
    pub result: Result<(), String>,
}

#[cfg(test)]
impl PowerPort for FakePower {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn perform(&mut self, action: SessionAction) -> Result<(), String> {
        self.calls.push(action);
        self.result.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_caps() -> SessionCapabilities {
        SessionCapabilities {
            lock: true,
            logout: true,
            suspend: true,
            reboot: true,
            shutdown: true,
        }
    }

    fn ui(caps: SessionCapabilities) -> PowerUi {
        PowerUi::new(caps)
    }

    #[test]
    fn labels_and_enablement_follow_capabilities() {
        assert_eq!(PowerButton::Sleep.label(), "Sleep");
        assert_eq!(PowerButton::Restart.label(), "Restart");
        assert_eq!(PowerButton::Shutdown.label(), "Shut Down");
        assert_eq!(PowerButton::Other.label(), "Other…");
        let none = SessionCapabilities::default();
        for b in [PowerButton::Sleep, PowerButton::Restart, PowerButton::Shutdown, PowerButton::Other] {
            assert!(!b.enabled(none));
            assert!(b.enabled(all_caps()));
        }
        let only_logout = SessionCapabilities { logout: true, ..Default::default() };
        assert!(PowerButton::Other.enabled(only_logout));
        assert!(!PowerButton::Sleep.enabled(only_logout));
    }

    #[test]
    fn sleep_performs_suspend_immediately() {
        let mut p = ui(all_caps());
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Sleep)), Effect::Perform(SessionAction::Suspend));
        assert_eq!(p.state(), &PowerState::Busy);
    }

    #[test]
    fn restart_requires_confirm_then_reboots() {
        let mut p = ui(all_caps());
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Restart)), Effect::None);
        assert_eq!(p.state(), &PowerState::Confirm(Confirmable::Restart));
        assert_eq!(p.handle(PowerEvent::Confirm), Effect::Perform(SessionAction::Reboot));
        assert_eq!(p.state(), &PowerState::Busy);
    }

    #[test]
    fn shutdown_requires_confirm_then_shuts_down() {
        let mut p = ui(all_caps());
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Shutdown)), Effect::None);
        assert_eq!(p.state(), &PowerState::Confirm(Confirmable::Shutdown));
        assert_eq!(p.handle(PowerEvent::Confirm), Effect::Perform(SessionAction::Shutdown));
    }

    #[test]
    fn cancel_and_escape_return_to_idle_without_a_call() {
        for ev in [PowerEvent::Cancel, PowerEvent::Escape] {
            let mut p = ui(all_caps());
            p.handle(PowerEvent::Click(PowerButton::Shutdown));
            assert_eq!(p.handle(ev.clone()), Effect::None);
            assert_eq!(p.state(), &PowerState::Idle);
            assert_eq!(p.handle(PowerEvent::Confirm), Effect::None);
        }
    }

    #[test]
    fn other_opens_list_and_logout_performs_logout() {
        let mut p = ui(all_caps());
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Other)), Effect::None);
        assert_eq!(p.state(), &PowerState::OtherOpen);
        assert_eq!(p.handle(PowerEvent::ClickLogout), Effect::Perform(SessionAction::Logout));
        assert_eq!(p.state(), &PowerState::Busy);
    }

    #[test]
    fn escape_closes_other_list_without_effect() {
        let mut p = ui(all_caps());
        p.handle(PowerEvent::Click(PowerButton::Other));
        assert_eq!(p.handle(PowerEvent::Escape), Effect::None);
        assert_eq!(p.state(), &PowerState::Idle);
        assert_eq!(p.handle(PowerEvent::ClickLogout), Effect::None);
    }

    #[test]
    fn disabled_actions_are_inert() {
        let mut p = ui(SessionCapabilities::default());
        for b in [PowerButton::Sleep, PowerButton::Restart, PowerButton::Shutdown, PowerButton::Other] {
            assert_eq!(p.handle(PowerEvent::Click(b)), Effect::None);
            assert_eq!(p.state(), &PowerState::Idle);
        }
        let mut no_logout = ui(SessionCapabilities { suspend: true, ..Default::default() });
        no_logout.handle(PowerEvent::Click(PowerButton::Other));
        assert_eq!(no_logout.state(), &PowerState::Idle);
        assert_eq!(no_logout.handle(PowerEvent::ClickLogout), Effect::None);
    }

    #[test]
    fn restart_without_capability_never_confirms() {
        let mut p = ui(SessionCapabilities { shutdown: true, ..Default::default() });
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Restart)), Effect::None);
        assert_eq!(p.state(), &PowerState::Idle);
    }

    #[test]
    fn busy_ignores_further_clicks_and_confirms() {
        let mut p = ui(all_caps());
        p.handle(PowerEvent::Click(PowerButton::Sleep));
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Sleep)), Effect::None);
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Other)), Effect::None);
        assert_eq!(p.handle(PowerEvent::ClickLogout), Effect::None);
        assert_eq!(p.handle(PowerEvent::Escape), Effect::None);
        assert_eq!(p.state(), &PowerState::Busy);
    }

    #[test]
    fn result_ok_returns_idle_and_err_shows_message() {
        let mut ok = ui(all_caps());
        ok.handle(PowerEvent::Click(PowerButton::Sleep));
        assert_eq!(ok.handle(PowerEvent::Result(Ok(()))), Effect::None);
        assert_eq!(ok.state(), &PowerState::Idle);

        let mut err = ui(all_caps());
        err.handle(PowerEvent::Click(PowerButton::Sleep));
        err.handle(PowerEvent::Result(Err("nope".into())));
        assert_eq!(err.state(), &PowerState::Error("nope".into()));
    }

    #[test]
    fn result_outside_busy_is_ignored() {
        let mut p = ui(all_caps());
        p.handle(PowerEvent::Result(Err("late".into())));
        assert_eq!(p.state(), &PowerState::Idle);
    }

    #[test]
    fn error_auto_clears_after_four_seconds_of_ticks() {
        let mut p = ui(all_caps());
        p.handle(PowerEvent::Click(PowerButton::Sleep));
        p.handle(PowerEvent::Result(Err("boom".into())));
        p.handle(PowerEvent::Tick(Duration::from_secs(3)));
        assert_eq!(p.state(), &PowerState::Error("boom".into()));
        p.handle(PowerEvent::Tick(Duration::from_millis(999)));
        assert_eq!(p.state(), &PowerState::Error("boom".into()));
        p.handle(PowerEvent::Tick(Duration::from_millis(1)));
        assert_eq!(p.state(), &PowerState::Idle);
    }

    #[test]
    fn error_can_be_dismissed_and_new_click_starts_fresh() {
        let mut p = ui(all_caps());
        p.handle(PowerEvent::Click(PowerButton::Sleep));
        p.handle(PowerEvent::Result(Err("boom".into())));
        assert_eq!(p.handle(PowerEvent::Click(PowerButton::Sleep)), Effect::Perform(SessionAction::Suspend));
        assert_eq!(p.state(), &PowerState::Busy);
    }

    #[test]
    fn tick_in_idle_is_inert() {
        let mut p = ui(all_caps());
        assert_eq!(p.handle(PowerEvent::Tick(Duration::from_secs(10))), Effect::None);
        assert_eq!(p.state(), &PowerState::Idle);
    }

    #[test]
    fn fake_power_records_calls_through_port() {
        let mut fake = FakePower { capabilities: all_caps(), calls: Vec::new(), result: Ok(()) };
        assert_eq!(fake.capabilities(), all_caps());
        assert_eq!(fake.perform(SessionAction::Reboot), Ok(()));
        assert_eq!(fake.calls, vec![SessionAction::Reboot]);
    }

    #[test]
    fn control_power_fails_closed_when_bus_cannot_connect() {
        let mut port = ControlPower::from_connect(|| Err::<ControlClient, _>("no bus"));
        assert_eq!(port.capabilities(), SessionCapabilities::default());
        assert_eq!(port.perform(SessionAction::Suspend), Err("control bus unavailable".into()));
    }
}
