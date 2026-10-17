use std::os::fd::RawFd;

use flamewm_api::FlameResult;
use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignalClient};
use flamewm_control_wire::ControlSignal;
use flamewm_dbus_reactor::BusKind;
use flamewm_settings_core::ControlTransport;

pub struct SettingsControl {
    client: ControlClient,
}

impl SettingsControl {
    pub fn connect_session() -> FlameResult<Self> {
        Ok(Self {
            client: ControlClient::connect(BusKind::Session)?,
        })
    }
}

impl ControlTransport for SettingsControl {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        self.client.call(&request)
    }
}

/// Session-bus connection that receives Flame Control change signals. It is receive-only: its
/// descriptor is registered with the UI reactor, which calls [`SettingsSignals::drain`] when the
/// bus becomes readable. No timer or per-frame polling is involved.
pub struct SettingsSignals {
    client: ControlSignalClient,
}

impl SettingsSignals {
    pub fn connect_session() -> FlameResult<Self> {
        Ok(Self {
            client: ControlSignalClient::connect_session()?,
        })
    }

    /// Descriptor to register as a readable wake source. It stays owned by this value.
    #[must_use]
    pub fn raw_fd(&self) -> RawFd {
        self.client.watch().fd
    }

    /// Reads the queued bus messages once and forwards each change signal in arrival order.
    pub fn drain(&self, handler: impl FnMut(ControlSignal)) -> FlameResult<usize> {
        self.client.on_ready(handler)
    }
}

/// One item queued from the reactor-side drain for the Settings UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsWake {
    Signal(ControlSignal),
    /// The signal connection failed. Live updates stop; the window keeps working.
    Lost(String),
}

/// The authoritative section a bus signal makes stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsScope {
    Settings,
    Displays,
    Shortcuts,
    Panels,
}

/// Revisions the Settings client has already projected. `None` means the section was never read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnownRevisions {
    pub settings: Option<u64>,
    pub displays_generation: Option<u64>,
    pub shortcuts: Option<u64>,
    pub panels: Option<u64>,
}

/// Maps one bus signal to the single section it makes stale.
///
/// Returns `None` when the signal carries the revision already projected (for example the echo of
/// this client's own mutation) or when the signal is not a settings-facing change. The display
/// transaction signal carries no snapshot revision, so it always reprojects displays.
#[must_use]
pub fn reproject_scope(signal: &ControlSignal, known: KnownRevisions) -> Option<SettingsScope> {
    let stale = |projected: Option<u64>, incoming: u64| projected != Some(incoming);
    match signal {
        ControlSignal::SettingsSnapshotChanged { snapshot } => {
            stale(known.settings, snapshot.revision).then_some(SettingsScope::Settings)
        }
        ControlSignal::DisplaysSnapshotChanged { snapshot } => {
            stale(known.displays_generation, snapshot.generation).then_some(SettingsScope::Displays)
        }
        ControlSignal::DisplayTransactionChanged { .. } => Some(SettingsScope::Displays),
        ControlSignal::ShortcutsChanged { revision } => {
            stale(known.shortcuts, *revision).then_some(SettingsScope::Shortcuts)
        }
        ControlSignal::PanelsSnapshotChanged { snapshot } => {
            stale(known.panels, snapshot.revision).then_some(SettingsScope::Panels)
        }
        _ => None,
    }
}

#[cfg(test)]
mod signal_tests {
    use flamewm_api::display::DisplaySnapshot;
    use flamewm_api::panels::PanelsSnapshot;
    use flamewm_api::settings::SettingsSnapshot;
    use flamewm_control_wire::ControlSignal;

    use super::{KnownRevisions, SettingsScope, reproject_scope};

    fn projected() -> KnownRevisions {
        KnownRevisions {
            settings: Some(7),
            displays_generation: Some(3),
            shortcuts: Some(4),
            panels: Some(5),
        }
    }

    fn scope_of(signal: &ControlSignal) -> Option<SettingsScope> {
        reproject_scope(signal, projected())
    }

    fn settings(revision: u64) -> ControlSignal {
        ControlSignal::SettingsSnapshotChanged {
            snapshot: SettingsSnapshot::new(revision),
        }
    }

    fn displays(generation: u64) -> ControlSignal {
        ControlSignal::DisplaysSnapshotChanged {
            snapshot: DisplaySnapshot {
                generation,
                outputs: Vec::new(),
                pending: None,
            },
        }
    }

    #[test]
    fn settings_signal_at_projected_revision_is_a_noop() {
        assert_eq!(scope_of(&settings(7)), None);
    }

    #[test]
    fn settings_signal_with_newer_revision_reprojects_only_settings() {
        assert_eq!(scope_of(&settings(8)), Some(SettingsScope::Settings));
    }

    #[test]
    fn displays_signal_at_projected_generation_is_a_noop() {
        assert_eq!(scope_of(&displays(3)), None);
    }

    #[test]
    fn displays_signal_with_new_generation_reprojects_displays() {
        assert_eq!(scope_of(&displays(4)), Some(SettingsScope::Displays));
    }

    #[test]
    fn display_transaction_signal_always_reprojects_displays() {
        let signal = ControlSignal::DisplayTransactionChanged {
            transaction: 9,
            state: "pending".to_owned(),
            deadline_ms: 0,
        };
        assert_eq!(scope_of(&signal), Some(SettingsScope::Displays));
    }

    #[test]
    fn shortcut_and_panel_revisions_gate_their_own_scope() {
        let same = ControlSignal::ShortcutsChanged { revision: 4 };
        let newer = ControlSignal::ShortcutsChanged { revision: 5 };
        assert_eq!(scope_of(&same), None);
        assert_eq!(scope_of(&newer), Some(SettingsScope::Shortcuts));

        let panels = ControlSignal::PanelsSnapshotChanged {
            snapshot: PanelsSnapshot {
                revision: 6,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            },
        };
        assert_eq!(scope_of(&panels), Some(SettingsScope::Panels));
    }

    #[test]
    fn unprojected_sections_reproject_on_first_signal() {
        assert_eq!(
            reproject_scope(&settings(1), KnownRevisions::default()),
            Some(SettingsScope::Settings)
        );
    }

    #[test]
    fn unrelated_signals_never_reproject() {
        assert_eq!(scope_of(&ControlSignal::StartMenuToggleRequested), None);
        assert_eq!(scope_of(&ControlSignal::WorkspacesChanged { revision: 2 }), None);
    }
}
