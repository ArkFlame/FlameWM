//! Event-driven Linux host integration providers.
//!
//! This crate owns the native D-Bus adapters, while the reactor-neutral lifecycle and snapshot
//! rules remain in `flamewm-integrations-core`.

#[cfg(feature = "pulse-audio")]
extern crate pulse as libpulse_binding;

pub mod audio_helper;
pub mod audio_protocol;
pub mod background;
pub mod backlight;
pub mod bluetooth;
pub mod device_watch;
pub mod icon_service;
pub mod icon_theme;
pub mod icons;
pub mod job_view;
pub mod launcher_entry;
pub mod media_store;
pub mod mpris;
pub mod network_manager;
pub mod network_slot;
pub mod notification_history;
pub mod notifications;
pub mod portal_file_chooser;
pub mod power;
pub mod power_profiles;
#[cfg(feature = "pulse-audio")]
pub mod pulse;
pub mod removable_mounts;
pub mod themes;

pub use icon_service::{
    IconJob, IconPriority, IconResult, IconService, IconSubmit, IconSubmitError,
};
pub use icon_theme::{DirListings, IconLookupIndex};
pub use icons::{IconKey, IconMetricsSnapshot, IconResolver};

pub use backlight::BacklightProvider;
pub use mpris::{MprisProvider, MprisProviderState};
pub use network_manager::{
    NetworkManagerProperties, NetworkManagerProvider, NetworkManagerProviderState,
};
pub use network_slot::NetworkManagerSlot;
pub use portal_file_chooser::PortalFileChooser;
pub use power::PowerProvider;
pub use power_profiles::{PowerProfile, PowerProfilesProvider, PowerProfilesSnapshot};

use flamewm_api::FlameResult;
use flamewm_api::system::{
    BrightnessAction, BrightnessSnapshot, MediaSnapshot, NetworkSnapshot, PowerSnapshot,
    SystemAction,
};
use flamewm_dbus_reactor::BusWatch;
use removable_mounts::RemovableMountWorker;

/// Which D-Bus source reported readiness to the desktop reactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxIntegrationSource {
    NetworkManager,
    Mpris,
    Power,
    PowerProfiles,
}

/// The single owner of Linux integration provider lifecycles.
pub struct LinuxIntegrationRuntime {
    network_manager: NetworkManagerSlot,
    mpris: MprisProvider,
    power: PowerProvider,
    power_profiles: PowerProfilesProvider,
    brightness: BacklightProvider,
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
            power_profiles: PowerProfilesProvider::connect()?,
            brightness: BacklightProvider::connect(),
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
        self.power_profiles.start();
        self.brightness.start();
        self.removable_mounts = Some(RemovableMountWorker::start());
        self.started = true;
        Ok(())
    }

    /// Stop publication and invalidate provider-local generations.
    pub fn stop(&mut self) {
        self.removable_mounts = None;
        self.brightness.stop();
        self.power_profiles.stop();
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
            LinuxIntegrationSource::PowerProfiles => self.power_profiles.watch(),
        }
    }

    /// Drain one bounded reactor turn from the selected native source.
    pub fn on_ready(&mut self, source: LinuxIntegrationSource) -> FlameResult<usize> {
        match source {
            LinuxIntegrationSource::NetworkManager => self.network_manager.on_ready(),
            LinuxIntegrationSource::Mpris => self.mpris.on_ready(),
            LinuxIntegrationSource::Power => self.power.on_ready(),
            LinuxIntegrationSource::PowerProfiles => self.power_profiles.on_ready(),
        }
    }

    /// Time until the next lazy MPRIS reconnect attempt; `None` while connected. The WM arms this
    /// as a reactor wake so an idle session picks up a bus that appears later.
    #[must_use]
    pub fn next_retry_in(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        self.mpris.next_retry_in(now)
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

    #[must_use]
    pub fn power_profiles_snapshot(&self) -> flamewm_api::system::PowerProfilesSnapshot {
        self.power_profiles.snapshot()
    }

    /// Reads the panel backlight on demand; the generation only moves when the observed
    /// percent changes, so a stale-key fence is stable between turns.
    pub fn brightness_snapshot(&mut self) -> BrightnessSnapshot {
        self.brightness.snapshot()
    }

    /// Routes one brightness action to the panel device.
    pub fn set_brightness(&mut self, action: BrightnessAction) -> FlameResult<()> {
        self.brightness
            .set_percent(action.percent, action.generation)
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
                self.power_profiles.refresh();
                Ok(())
            }
            SystemAction::SetPowerProfile(profile) => self.power_profiles.set_active(profile),
            SystemAction::BatteryMenuClosed => {
                self.power.set_battery_menu_open(false);
                Ok(())
            }
            SystemAction::SetBrightness(action) => self.set_brightness(action),
            SystemAction::SetVolume(_) | SystemAction::SetMute(_) => {
                Err(flamewm_api::FlameError::new(
                    flamewm_api::ErrorCode::Unsupported,
                    "system action is not a D-Bus integration action",
                ))
            }
        }
    }
}
