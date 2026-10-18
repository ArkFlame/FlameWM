//! J08 shell convergence: audio/network UX view helpers. Core snapshot
//! types (`StatusViews`, `StatusPopovers`, sizes) stay in the crate root;
//! this module only adds tab/settings/query/scroll helpers.

pub use flamewm_ui_core::range::RangeSpec;
pub use flamewm_ui_core::scroll::ScrollStore;
pub use flamewm_ui_core::virtual_list::VirtualListModel;

/// Audio Devices/Applications tab selection. Pure view state owned by shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioTab {
    #[default]
    Devices,
    Applications,
}

/// Persisted audio UX settings. `raise_maximum` caps the slider at 100/150
/// and persists via the `audioRaiseMaximum` settings key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioSettings {
    pub raise_maximum: bool,
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
        needle.is_empty() || label.to_lowercase().contains(&needle)
    }
}
