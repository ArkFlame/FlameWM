//! Event-driven Linux host integration providers.
//!
//! This crate owns the native D-Bus adapters, while the reactor-neutral lifecycle and snapshot
//! rules remain in `flamewm-integrations-core`.

extern crate pulse as libpulse_binding;

pub mod icon_service;
pub mod icon_theme;
pub mod icons;
pub mod device_watch;
pub mod job_view;
pub mod media_store;
pub mod mpris;
pub mod network_manager;
pub mod network_slot;
pub mod notifications;
pub mod portal_file_chooser;
pub mod power;
pub mod pulse;
pub mod removable_mounts;
pub mod themes;

pub use icon_service::{
    IconJob, IconPriority, IconResult, IconService, IconSubmit, IconSubmitError,
};
pub use icon_theme::IconLookupIndex;
pub use icons::{IconKey, IconMetricsSnapshot, IconResolver};

pub use mpris::{MprisProvider, MprisProviderState};
pub use network_manager::{
    NetworkManagerProperties, NetworkManagerProvider, NetworkManagerProviderState,
};
pub use network_slot::NetworkManagerSlot;
pub use portal_file_chooser::PortalFileChooser;
pub use power::PowerProvider;

use flamewm_api::FlameResult;
use flamewm_api::system::{MediaSnapshot, NetworkSnapshot, PowerSnapshot, SystemAction};
use flamewm_dbus_reactor::BusWatch;
use removable_mounts::RemovableMountWorker;

/// Which D-Bus source reported readiness to the desktop reactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxIntegrationSource {
    NetworkManager,
    Mpris,
    Power,
}

/// The single owner of Linux integration provider lifecycles.
pub struct LinuxIntegrationRuntime {
    network_manager: NetworkManagerSlot,
    mpris: MprisProvider,
    power: PowerProvider,
    removable_mounts: Option<RemovableMountWorker>,
    started: bool,
}

impl LinuxIntegrationRuntime {
    /// Connect both optional providers without creating a second event loop.
    pub fn connect() -> FlameResult<Self> {
        Ok(Self {
            network_manager: NetworkManagerSlot::connect()?,
            mpris: MprisProvider::connect()?,
            power: PowerProvider::connect()?,
            removable_mounts: None,
            started: false,
        })
    }

    /// Subscribe to native owner/property events.
    pub fn start(&mut self) -> FlameResult<()> {
        if self.started {
            return Ok(());
        }
        self.network_manager.start()?;
        self.mpris.start()?;
        self.power.start();
        self.removable_mounts = Some(RemovableMountWorker::start());
        self.started = true;
        Ok(())
    }

    /// Stop publication and invalidate provider-local generations.
    pub fn stop(&mut self) {
        self.removable_mounts = None;
        self.power.stop();
        self.mpris.stop();
        self.network_manager.stop();
        self.started = false;
    }

    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub fn network_manager(&self) -> Option<&NetworkManagerProvider> {
        self.network_manager.provider()
    }

    #[must_use]
    pub fn mpris(&self) -> &MprisProvider {
        &self.mpris
    }

    #[must_use]
    pub fn watch(&self, source: LinuxIntegrationSource) -> BusWatch {
        match source {
            LinuxIntegrationSource::NetworkManager => self.network_manager.watch(),
            LinuxIntegrationSource::Mpris => self.mpris.watch(),
            LinuxIntegrationSource::Power => self.power.watch(),
        }
    }

    /// Drain one bounded reactor turn from the selected native source.
    pub fn on_ready(&mut self, source: LinuxIntegrationSource) -> FlameResult<usize> {
        match source {
            LinuxIntegrationSource::NetworkManager => self.network_manager.on_ready(),
            LinuxIntegrationSource::Mpris => self.mpris.on_ready(),
            LinuxIntegrationSource::Power => self.power.on_ready(),
        }
    }

    /// Reconcile event-coalesced provider state without polling.
    pub fn reconcile(&mut self) -> FlameResult<bool> {
        let network_changed = self.network_manager.reconcile()?;
        let media_changed = self.mpris.reconcile();
        Ok(network_changed || media_changed)
    }

    #[must_use]
    pub fn network_snapshot(&self) -> NetworkSnapshot {
        self.network_manager.snapshot()
    }

    #[must_use]
    pub fn media_snapshot(&self) -> MediaSnapshot {
        self.mpris.snapshot()
    }

    #[must_use]
    pub fn power_snapshot(&self) -> PowerSnapshot {
        self.power.snapshot()
    }

    /// One CPU sample while the battery menu is open. Returns true when the published value changed.
    pub fn sample_power_cpu(&mut self) -> bool {
        self.power.sample_cpu()
    }

    /// Route every optional integration command to its native state owner.
    pub fn perform_action(
        &mut self,
        action: SystemAction,
        network: &NetworkSnapshot,
        media: &MediaSnapshot,
    ) -> FlameResult<()> {
        match action {
            SystemAction::SetWifiEnabled(_)
            | SystemAction::ConnectKnown { .. }
            | SystemAction::ConnectWifi { .. }
            | SystemAction::SubmitNetworkSecret { .. }
            | SystemAction::CancelNetworkSecret { .. }
            | SystemAction::Disconnect
            | SystemAction::Scan => self.network_manager.perform_action(action, network),
            SystemAction::Play { .. }
            | SystemAction::Pause { .. }
            | SystemAction::PlayPause { .. }
            | SystemAction::Next { .. }
            | SystemAction::Previous { .. } => self.mpris.perform_action(action, media),
            SystemAction::BatteryMenuOpened => {
                self.power.set_battery_menu_open(true);
                Ok(())
            }
            SystemAction::BatteryMenuClosed => {
                self.power.set_battery_menu_open(false);
                Ok(())
            }
            SystemAction::SetVolume(_) | SystemAction::SetMute(_) => {
                Err(flamewm_api::FlameError::new(
                    flamewm_api::ErrorCode::Unsupported,
                    "system action is not a D-Bus integration action",
                ))
            }
        }
    }
}
