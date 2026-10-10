//! J08 shell convergence: audio/network UX view helpers. Core snapshot
//! types (`StatusViews`, `StatusPopovers`, sizes) stay in the crate root;
//! this module only adds tab/settings/query/scroll/measure helpers plus the
//! friendly endpoint description.

use flamewm_api::system::AudioSnapshot;
pub use flamewm_ui_core::range::RangeSpec;
pub use flamewm_ui_core::scroll::ScrollStore;
pub use flamewm_ui_core::virtual_list::VirtualListModel;

/// Friendly endpoint description for the default endpoint; falls back to a
/// generic label. The raw sink id is never returned as primary text.
#[must_use]
pub fn friendly_audio_label(audio: &AudioSnapshot) -> String {
    if let Some(default) = audio.endpoints().iter().find(|e| e.is_default) {
        if !default.description.is_empty() {
            return default.description.clone();
        }
        if !default.name.is_empty() && !looks_like_raw_sink_id(&default.name) {
            return default.name.clone();
        }
        return "Default output".to_owned();
    }
    if !audio.sink_name.is_empty() && !looks_like_raw_sink_id(&audio.sink_name) {
        return audio.sink_name.clone();
    }
    "Audio".to_owned()
}

fn looks_like_raw_sink_id(name: &str) -> bool {
    // Raw PulseAudio sink ids look like `alsa_output.pci-0000_...analog-stereo`.
    name.contains("alsa_output.") || name.contains("bluez_output.") || name.contains("oss_output.")
}

/// Audio Devices/Applications tab selection. Pure view state owned by shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioTab {
    #[default]
    Devices,
    Applications,
}

/// Persisted audio UX settings. `raise_maximum` caps the slider at 100/150
/// and persists via the `audioRaiseMaximum` settings key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSettings {
    pub raise_maximum: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            raise_maximum: false,
        }
    }
}

impl AudioSettings {
    pub const SETTINGS_KEY: &str = "audioRaiseMaximum";

    #[must_use]
    pub fn max_percent(self) -> u8 {
        if self.raise_maximum { 150 } else { 100 }
    }

    /// Slider spec for a row volume under the current cap.
    #[must_use]
    pub fn slider_spec(self, volume_percent: u8) -> RangeSpec {
        RangeSpec::new(
            0.0,
            f32::from(self.max_percent()),
            1.0,
            f32::from(volume_percent),
        )
    }

    /// Clamp a requested percent to the current cap.
    #[must_use]
    pub fn clamp_percent(self, percent: u8) -> u8 {
        percent.min(self.max_percent())
    }
}

/// Scroll/virtualization window over a full row list. Backed by
/// `VirtualListModel`; no model truncation.
#[must_use]
pub fn visible_window(
    total_rows: usize,
    row_height: u32,
    viewport_height: u32,
    scroll_offset: u32,
) -> (usize, usize) {
    VirtualListModel {
        row_count: total_rows,
        row_height,
        viewport_height,
        scroll_offset,
    }
    .visible_range()
}

/// Measured popover height: header plus one row per visible row, clamped to
/// a viewport cap so overflow scrolls instead of growing the surface.
#[must_use]
pub fn measured_popover_size(
    base_width: i32,
    header_height: i32,
    row_height: i32,
    rows: usize,
    max_height: i32,
) -> flamewm_api::Size {
    let content = header_height + row_height * rows.max(1) as i32;
    flamewm_api::Size::new(base_width, content.min(max_height).max(header_height))
}

/// Measured start-submenu height for `count` rows at `row_height` plus
/// padding, clamped to the work-area height.
#[must_use]
pub fn measured_start_submenu_size(
    width: i32,
    row_height: i32,
    rows: usize,
    padding: i32,
    max_height: i32,
) -> flamewm_api::Size {
    let content = padding + row_height * rows as i32;
    flamewm_api::Size::new(width, content.clamp(padding.max(1), max_height.max(1)))
}

/// Network filter over the full AP snapshot plus Connected/Available
/// grouping. The filter matches SSID/label case-insensitively; grouping
/// never drops rows, it only partitions them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkQuery {
    pub filter: String,
}

impl NetworkQuery {
    #[must_use]
    pub fn matches(&self, label: &str) -> bool {
        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        label.to_lowercase().contains(&needle)
    }
}

/// Active-AP predicate shared by shell network views.
///
/// `NetworkSnapshot::active_access_point_path` is the wireless device's
/// `ActiveAccessPoint` object path. It is compared to the AP object path
/// only, never to `active_path` (the ActiveConnection) and never by SSID,
/// so duplicate SSIDs yield exactly one connected row.
#[must_use]
pub fn network_ap_connected(
    access_point_path: &str,
    snapshot: &flamewm_api::system::NetworkSnapshot,
) -> bool {
    !snapshot.active_access_point_path.is_empty()
        && access_point_path == snapshot.active_access_point_path
        && snapshot
            .access_points
            .iter()
            .any(|ap| ap.path == access_point_path)
}
