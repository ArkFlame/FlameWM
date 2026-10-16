//! Fail-soft ownership of the optional NetworkManager system-bus provider.
//!
//! An unreachable system bus must not abort the desktop: the slot degrades to
//! `Unavailable`, which publishes the same snapshot as a missing NetworkManager.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use flamewm_api::ports::FdEvents;
use flamewm_api::system::{NetworkKind, NetworkSnapshot, ServiceAvailability, SystemAction};
use flamewm_api::{FlameError, FlameResult};
use flamewm_dbus_reactor::BusWatch;

use crate::network_manager::NetworkManagerProvider;

/// NetworkManager provider slot. `Unavailable` keeps the reason and a never-ready fd so the
/// reactor registration shape is unchanged without any timer or polling.
pub enum NetworkManagerSlot {
    Connected(NetworkManagerProvider),
    Unavailable(Unavailable),
}

pub struct Unavailable {
    reason: String,
    _peer: UnixStream,
    dormant: UnixStream,
}

impl NetworkManagerSlot {
    pub fn connect() -> FlameResult<Self> {
        Self::connect_with(NetworkManagerProvider::connect)
    }

    /// Seam for tests: a failing connector yields `Unavailable` instead of an error.
    pub fn connect_with(
        connect: impl FnOnce() -> FlameResult<NetworkManagerProvider>,
    ) -> FlameResult<Self> {
        match connect() {
            Ok(provider) => Ok(Self::Connected(provider)),
            Err(error) => {
                let (peer, dormant) = UnixStream::pair()
                    .map_err(|io| FlameError::unavailable(format!("socketpair failed: {io}")))?;
                Ok(Self::Unavailable(Unavailable {
                    reason: error.to_string(),
                    _peer: peer,
                    dormant,
                }))
            }
        }
    }

    pub fn start(&mut self) -> FlameResult<()> {
        match self {
            Self::Connected(provider) => provider.start(),
            Self::Unavailable(_) => Ok(()),
        }
    }

    pub fn stop(&mut self) {
        if let Self::Connected(provider) = self {
            provider.stop();
        }
    }

    #[must_use]
    pub fn provider(&self) -> Option<&NetworkManagerProvider> {
        match self {
            Self::Connected(provider) => Some(provider),
            Self::Unavailable(_) => None,
        }
    }

    pub fn provider_mut(&mut self) -> Option<&mut NetworkManagerProvider> {
        match self {
            Self::Connected(provider) => Some(provider),
            Self::Unavailable(_) => None,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> NetworkSnapshot {
        match self {
            Self::Connected(provider) => provider.snapshot(),
            Self::Unavailable(_) => NetworkSnapshot {
                availability: ServiceAvailability::Unavailable,
                kind: NetworkKind::Unavailable,
                ..NetworkSnapshot::default()
            },
        }
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        match self {
            Self::Connected(provider) => provider.watch(),
            Self::Unavailable(unavailable) => BusWatch {
                fd: unavailable.dormant.as_raw_fd(),
                events: FdEvents::READABLE,
            },
        }
    }

    pub fn on_ready(&mut self) -> FlameResult<usize> {
        match self {
            Self::Connected(provider) => provider.on_ready(),
            Self::Unavailable(_) => Ok(0),
        }
    }

    pub fn reconcile(&mut self) -> FlameResult<bool> {
        match self {
            Self::Connected(provider) => provider.reconcile(),
            Self::Unavailable(_) => Ok(false),
        }
    }

    pub fn perform_action(
        &mut self,
        action: SystemAction,
        snapshot: &NetworkSnapshot,
    ) -> FlameResult<()> {
        match self {
            Self::Connected(provider) => provider.perform_action(action, snapshot),
            Self::Unavailable(unavailable) => Err(FlameError::unavailable(format!(
                "NetworkManager unavailable: {}",
                unavailable.reason
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use flamewm_api::ErrorCode;

    use super::*;

    const CHILD_ENV: &str = "FLAMEWM_NM_ABSENT_BUS_CHILD";

    fn failing_connector() -> FlameResult<NetworkManagerProvider> {
        Err(FlameError::unavailable(
            "D-Bus connection failed: Failed to connect to socket /nonexistent/flamewm-test-bus",
        ))
    }

    #[test]
    fn failing_connector_yields_unavailable_without_error_or_panic() {
        let mut slot = NetworkManagerSlot::connect_with(failing_connector)
            .expect("unreachable system bus must not fail the constructor");

        assert!(slot.provider().is_none());
        assert_eq!(
            slot.snapshot().availability,
            ServiceAvailability::Unavailable
        );
        assert_eq!(slot.snapshot().kind, NetworkKind::Unavailable);
        slot.start().expect("start is a no-op when unavailable");
        assert!(!slot.reconcile().expect("reconcile is a no-op"));
        assert_eq!(slot.on_ready().expect("on_ready is a no-op"), 0);
        slot.stop();
    }

    #[test]
    fn unavailable_slot_rejects_network_actions_with_unavailable_code() {
        let mut slot = NetworkManagerSlot::connect_with(failing_connector).unwrap();
        let snapshot = slot.snapshot();

        let error = slot
            .perform_action(SystemAction::Scan, &snapshot)
            .expect_err("actions must not silently succeed without NetworkManager");

        assert_eq!(error.code, ErrorCode::Unavailable);
        assert!(error.message.contains("D-Bus connection failed"));
    }

    #[test]
    fn unavailable_slot_watch_is_a_valid_never_ready_fd() {
        let slot = NetworkManagerSlot::connect_with(failing_connector).unwrap();
        let watch = slot.watch();

        assert!(watch.fd >= 0, "reactor registration needs a real fd");
        assert_eq!(watch.events, FdEvents::READABLE);
    }

    #[test]
    fn absent_system_bus_degrades_to_unavailable() {
        if std::env::var_os(CHILD_ENV).is_some() {
            let mut slot = NetworkManagerSlot::connect()
                .expect("absent system bus must not fail the constructor");
            assert_eq!(
                slot.snapshot().availability,
                ServiceAvailability::Unavailable
            );
            slot.start().expect("start degrades");
            assert_eq!(slot.on_ready().expect("on_ready degrades"), 0);
            return;
        }
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new(exe)
            .args([
                "--exact",
                "network_slot::tests::absent_system_bus_degrades_to_unavailable",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                "unix:path=/nonexistent/flamewm-test-bus",
            )
            .status()
            .expect("spawn child test process");
        assert!(status.success(), "child exited with {status}");
    }
}
