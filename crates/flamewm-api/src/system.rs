//! Typed shell-facing state for optional host integrations.

/// Highest volume any audio path accepts: the shell, the platform media keys, the helper
/// protocol and the `PulseAudio` backend all clamp to this one bound.
pub const MAX_VOLUME_PERCENT: u8 = 150;
/// Volume cap without "raise maximum volume" (over-amplification).
pub const BASE_VOLUME_PERCENT: u8 = 100;

/// Volume cap for the shared "raise maximum volume" value: 150 when raised, else 100.
#[must_use]
pub fn volume_cap_percent(raise_maximum: bool) -> u8 {
    if raise_maximum {
        MAX_VOLUME_PERCENT
    } else {
        BASE_VOLUME_PERCENT
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServiceAvailability {
    #[default]
    Unknown,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkKind {
    #[default]
    Unavailable,
    Disconnected,
    Connecting,
    Wired,
    Wireless,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkAccessPointSnapshot {
    pub path: String,
    pub ssid: String,
    pub strength_percent: u8,
    pub secured: bool,
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSecretRequestSnapshot {
    pub request_id: u64,
    pub access_point_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSnapshot {
    pub availability: ServiceAvailability,
    pub generation: u64,
    pub kind: NetworkKind,
    pub label: String,
    pub strength_percent: u8,
    pub wifi_enabled: bool,
    pub networking_enabled: bool,
    pub active_path: String,
    /// Object path of the access point the wireless device is connected to (empty when none).
    /// Additive: defaults to empty; compare against `NetworkAccessPointSnapshot::path`.
    pub active_access_point_path: String,
    pub access_points: Vec<NetworkAccessPointSnapshot>,
    pub pending_secret: Option<NetworkSecretRequestSnapshot>,
}

impl Default for NetworkSnapshot {
    fn default() -> Self {
        Self {
            availability: ServiceAvailability::Unknown,
            generation: 0,
            kind: NetworkKind::Unavailable,
            label: String::new(),
            strength_percent: 0,
            wifi_enabled: false,
            networking_enabled: false,
            active_path: String::new(),
            active_access_point_path: String::new(),
            access_points: Vec::new(),
            pending_secret: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSnapshot {
    pub availability: ServiceAvailability,
    pub generation: u64,
    pub active_bus_name: String,
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub playback: PlaybackState,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
}

impl Default for MediaSnapshot {
    fn default() -> Self {
        Self {
            availability: ServiceAvailability::Unknown,
            generation: 0,
            active_bus_name: String::new(),
            identity: String::new(),
            title: String::new(),
            artist: String::new(),
            playback: PlaybackState::Unavailable,
            can_play: false,
            can_pause: false,
            can_next: false,
            can_previous: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioEndpointKind {
    #[default]
    Sink,
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEndpointSnapshot {
    pub id: u32,
    pub kind: AudioEndpointKind,
    pub name: String,
    pub description: String,
    pub volume_percent: u8,
    pub muted: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioStreamSnapshot {
    pub id: u32,
    pub endpoint_id: u32,
    pub name: String,
    pub volume_percent: u8,
    pub muted: bool,
    /// Sanitized `application.name` from the stream proplist, when the producer knows it.
    pub app_name: Option<String>,
    /// Sanitized `application.id` from the stream proplist.
    pub app_id: Option<String>,
    /// Sanitized `application.icon_name` from the stream proplist.
    pub icon_name: Option<String>,
    /// Sanitized `application.process.binary` from the stream proplist.
    pub process_binary: Option<String>,
    /// Sanitized `media.name` from the stream proplist.
    pub media_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PowerState {
    Charging,
    Discharging,
    Full,
    Empty,
    #[default]
    Unknown,
}

/// Aggregate `UPower` `DisplayDevice` state. `cpu_percent` is sampled only while the battery menu is open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PowerSnapshot {
    pub available: ServiceAvailability,
    pub present: bool,
    pub percent: u8,
    pub state: PowerState,
    pub cpu_percent: Option<u8>,
    /// Battery energy flow in integer milliwatts; `None` unless charging or discharging with a positive rate.
    pub power_rate_mw: Option<u32>,
}

/// Power profile names from `net.hadess.PowerProfiles`. Unknown names are never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    PowerSaver,
    Balanced,
    Performance,
}

impl PowerProfile {
    pub const ALL: [Self; 3] = [Self::PowerSaver, Self::Balanced, Self::Performance];

    /// Wire name used by both `net.hadess.PowerProfiles` and `org.freedesktop.UPower.PowerProfiles`.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    #[must_use]
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.wire() == name)
    }
}

/// Published profile state. `profiles` keeps service order with unknown and duplicate names removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerProfilesSnapshot {
    pub available: bool,
    pub active: Option<PowerProfile>,
    pub profiles: Vec<PowerProfile>,
}

impl PowerProfilesSnapshot {
    /// The control is shown only when a service answered and advertised at least one profile.
    #[must_use]
    pub fn tile_visible(&self) -> bool {
        self.available && !self.profiles.is_empty()
    }
}

/// One `BlueZ` write on a device: `Pair`, `Trusted` property, or `Adapter1.RemoveDevice`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothDeviceOp {
    Pair,
    SetTrusted(bool),
    Remove,
}

/// Device write. `address` is a validated `AA:BB:CC:DD:EE:FF` string; build with [`Self::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothDeviceAction {
    pub op: BluetoothDeviceOp,
    pub address: String,
}

impl BluetoothDeviceAction {
    /// Returns `None` unless `address` is six colon-separated hex octets.
    #[must_use]
    pub fn new(op: BluetoothDeviceOp, address: &str) -> Option<Self> {
        let bytes = address.as_bytes();
        let ok = bytes.len() == 17
            && bytes.iter().enumerate().all(|(index, byte)| {
                if index % 3 == 2 {
                    *byte == b':'
                } else {
                    byte.is_ascii_hexdigit()
                }
            });
        ok.then(|| Self {
            op,
            address: address.to_owned(),
        })
    }
}

/// Backlight state of the panel device. `percent` is `0..=100`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BrightnessSnapshot {
    pub availability: ServiceAvailability,
    pub generation: u64,
    pub percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrightnessAction {
    pub percent: u8,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSnapshot {
    pub availability: ServiceAvailability,
    pub generation: u64,
    pub server_generation: u64,
    pub sink_name: String,
    pub volume_percent: u8,
    pub muted: bool,
    pub endpoints: Vec<AudioEndpointSnapshot>,
    pub streams: Vec<AudioStreamSnapshot>,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            availability: ServiceAvailability::Unknown,
            generation: 0,
            server_generation: 0,
            sink_name: String::new(),
            volume_percent: 0,
            muted: false,
            endpoints: Vec::new(),
            streams: Vec::new(),
        }
    }
}

impl AudioSnapshot {
    #[must_use]
    pub fn with_items(
        mut self,
        endpoints: Vec<AudioEndpointSnapshot>,
        streams: Vec<AudioStreamSnapshot>,
    ) -> Self {
        self.endpoints = endpoints;
        self.streams = streams;
        self
    }

    #[must_use]
    pub fn endpoints(&self) -> &[AudioEndpointSnapshot] {
        &self.endpoints
    }

    #[must_use]
    pub fn streams(&self) -> &[AudioStreamSnapshot] {
        &self.streams
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTarget {
    Endpoint { id: u32, kind: AudioEndpointKind },
    Stream { id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioVolumeAction {
    pub target: AudioTarget,
    pub percent: u8,
    pub generation: u64,
    pub server_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioMuteAction {
    pub target: AudioTarget,
    pub muted: bool,
    pub generation: u64,
    pub server_generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemSnapshot {
    pub revision: u64,
    pub network: NetworkSnapshot,
    pub media: MediaSnapshot,
    pub audio: AudioSnapshot,
    pub power: PowerSnapshot,
    pub power_profiles: PowerProfilesSnapshot,
    pub brightness: BrightnessSnapshot,
}

#[derive(Clone, PartialEq, Eq)]
pub enum SystemAction {
    SetWifiEnabled(bool),
    ConnectKnown {
        access_point_path: String,
    },
    ConnectWifi {
        access_point_path: String,
        generation: u64,
    },
    SubmitNetworkSecret {
        request_id: u64,
        generation: u64,
        secret: String,
    },
    CancelNetworkSecret {
        request_id: u64,
        generation: u64,
    },
    Disconnect,
    Scan,
    Play {
        bus_name: String,
    },
    Pause {
        bus_name: String,
    },
    PlayPause {
        bus_name: String,
    },
    Next {
        bus_name: String,
    },
    Previous {
        bus_name: String,
    },
    SetVolume(AudioVolumeAction),
    SetMute(AudioMuteAction),
    SetBrightness(BrightnessAction),
    SetPowerProfile(PowerProfile),
    BluetoothDevice(BluetoothDeviceAction),
    BatteryMenuOpened,
    BatteryMenuClosed,
}

impl core::fmt::Debug for SystemAction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SetWifiEnabled(enabled) => {
                f.debug_tuple("SetWifiEnabled").field(enabled).finish()
            }
            Self::ConnectKnown { access_point_path } => f
                .debug_struct("ConnectKnown")
                .field("access_point_path", access_point_path)
                .finish(),
            Self::ConnectWifi {
                access_point_path,
                generation,
            } => f
                .debug_struct("ConnectWifi")
                .field("access_point_path", access_point_path)
                .field("generation", generation)
                .finish(),
            Self::SubmitNetworkSecret {
                request_id,
                generation,
                ..
            } => f
                .debug_struct("SubmitNetworkSecret")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("secret", &"<redacted>")
                .finish(),
            Self::CancelNetworkSecret {
                request_id,
                generation,
            } => f
                .debug_struct("CancelNetworkSecret")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::Disconnect => f.write_str("Disconnect"),
            Self::Scan => f.write_str("Scan"),
            Self::Play { bus_name } => f.debug_struct("Play").field("bus_name", bus_name).finish(),
            Self::Pause { bus_name } => {
                f.debug_struct("Pause").field("bus_name", bus_name).finish()
            }
            Self::PlayPause { bus_name } => f
                .debug_struct("PlayPause")
                .field("bus_name", bus_name)
                .finish(),
            Self::Next { bus_name } => f.debug_struct("Next").field("bus_name", bus_name).finish(),
            Self::Previous { bus_name } => f
                .debug_struct("Previous")
                .field("bus_name", bus_name)
                .finish(),
            Self::SetVolume(action) => f.debug_tuple("SetVolume").field(action).finish(),
            Self::SetMute(action) => f.debug_tuple("SetMute").field(action).finish(),
            Self::SetBrightness(action) => f.debug_tuple("SetBrightness").field(action).finish(),
            Self::SetPowerProfile(profile) => {
                f.debug_tuple("SetPowerProfile").field(profile).finish()
            }
            Self::BluetoothDevice(action) => {
                f.debug_tuple("BluetoothDevice").field(action).finish()
            }
            Self::BatteryMenuOpened => f.write_str("BatteryMenuOpened"),
            Self::BatteryMenuClosed => f.write_str("BatteryMenuClosed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_network_action_debug_redacts_secret() {
        let action = SystemAction::SubmitNetworkSecret {
            request_id: 7,
            generation: 3,
            secret: "not-for-logs".to_owned(),
        };
        let debug = format!("{action:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("not-for-logs"));
    }
}
