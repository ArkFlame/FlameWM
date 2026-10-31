use super::*;

/// Snapshot projection for the helper: one [`ControlClient`] fetch round
/// over the System domain only. No secret is ever carried: secrets travel
/// via the `SecretAgent` snapshot, never in this struct or the protocol.
#[derive(Debug, Clone)]
pub struct QuickControlSnapshot {
    pub system: flamewm_api::system::SystemSnapshot,
    pub audio_tab: AudioTab,
    pub audio_settings: AudioSettings,
    /// Audio row whose slider holds the pointer (press..release).
    pub audio_drag_slot: Option<usize>,
    /// Audio row-list scroll offset in px, moved by wheel `scroll` events.
    pub audio_scroll_offset: u32,
    pub network_query: NetworkQuery,
    pub network_scroll_offset: u32,
    pub network_secret_masked_len: usize,
    /// Ephemeral helper-only editor value; never projected except as a mask.
    pub network_secret: String,
    pub network_secret_request: Option<(u64, u64)>,
    pub network_secret_focused: bool,
    pub network_filter_focused: bool,
    pub network_secret_error: bool,
    pub network_secret_submit_id: Option<u64>,
    /// Presentation settings for the shared menu background and popup backdrop.
    pub settings: SettingsSnapshot,
    /// Clock calendar view state; changed only by `calendar.*` actions.
    pub calendar: crate::calendar::CalendarState,
    /// Main-shell weather view (settings + state), replaced by `Command::Weather`.
    pub weather: WeatherFeed,
    /// Location picker session in this popup.
    pub weather_search: LocationSearch,
}

impl QuickControlSnapshot {
    /// Every field at its default around `system`.
    #[must_use]
    pub fn new(system: flamewm_api::system::SystemSnapshot) -> Self {
        Self {
            system,
            audio_tab: AudioTab::default(),
            audio_settings: AudioSettings::default(),
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery::default(),
            network_scroll_offset: 0,
            calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
            weather: WeatherFeed::default(),
            weather_search: LocationSearch::default(),
            network_secret_masked_len: 0,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
        }
    }

    /// Replaces the popup's weather view. A changed location or enabled flag ends the picker session.
    pub fn apply_weather_feed(&mut self, feed: WeatherFeed) {
        if feed.settings.location != self.weather.settings.location
            || feed.settings.enabled != self.weather.settings.enabled
        {
            self.weather_search.reset();
        }
        self.weather = feed;
    }

    pub fn load(client: &ControlClient) -> Result<Self, HostError> {
        match client.call(&ControlRequest::GetSystem) {
            Ok(flamewm_control_core::ControlResponse::System(system)) => Ok(Self::new(system)),
            Ok(response) => Err(HostError::Control(format!(
                "GetSystem returned {response:?}"
            ))),
            Err(error) => Err(HostError::Control(format!(
                "GetSystem failed: {}",
                error.message
            ))),
        }
    }

    pub fn refresh_settings(&mut self, client: &ControlClient) {
        if let Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) =
            client.call(&ControlRequest::GetSettings)
        {
            self.store_settings(snapshot);
        }
    }

    /// Stores one settings snapshot: presentation for the shared menu background and the
    /// audio tab/settings projection.
    pub fn store_settings(&mut self, snapshot: SettingsSnapshot) {
        if let Some(SettingValue::Boolean(raise)) = snapshot.values.get(AudioSettings::SETTINGS_KEY)
        {
            self.audio_settings.raise_maximum = *raise;
        }
        self.settings = snapshot;
    }
}
