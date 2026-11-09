//! Single accessor for the shared `accent` setting.
//!
//! Every active, selected and focus colour that derives from the live accent routes through here.
//! The value comes from `flamewm_api::settings::accent_rgb`, so fallback and hex/name parsing cannot
//! drift between surfaces. No state is cached: each call is a pure function of the snapshot.

use flamewm_api::settings::SettingsSnapshot;

use crate::Color;

/// Live accent as opaque `0x00RRGGBB`. A missing or invalid setting yields the product default.
#[must_use]
pub fn accent(snapshot: &SettingsSnapshot) -> Color {
    Color(flamewm_api::settings::accent_rgb(snapshot))
}

/// Live accent as `(r, g, b, a)` with the given alpha, for tint tokens such as `--accent-soft`.
#[must_use]
pub fn accent_rgba(snapshot: &SettingsSnapshot, alpha: u8) -> (u8, u8, u8, u8) {
    let rgb = accent(snapshot).0;
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, alpha)
}

/// Hover, pressed and muted shades derived from one accent. Each channel is lifted toward white
/// for hover and scaled down for pressed and muted, so every state follows the live accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shades {
    pub hover: Color,
    pub pressed: Color,
    pub muted: Color,
}

#[must_use]
pub fn shades(accent: Color) -> Shades {
    let map = |f: fn(u16) -> u16| {
        let channel = |shift: u32| f(u16::from((accent.0 >> shift) as u8)) as u8;
        Color((u32::from(channel(16)) << 16) | (u32::from(channel(8)) << 8) | u32::from(channel(0)))
    };
    Shades {
        hover: map(|c| c + (255 - c) * 12 / 100),
        pressed: map(|c| c * 80 / 100),
        muted: map(|c| c * 42 / 100),
    }
}

#[cfg(test)]
mod tests {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};

    use super::{accent, accent_rgba, shades};
    use crate::Color;

    fn with_accent(value: &str) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(0);
        snapshot
            .values
            .insert("accent".into(), SettingValue::Text(value.into()));
        snapshot
    }

    #[test]
    fn default_accent_rgba_is_unchanged_for_every_alpha_step() {
        // Pixel contract: the product default must produce exactly the channels the token
        // projections used before this accessor existed (#3DAEE9 with the stylesheet alpha steps).
        let default = SettingsSnapshot::new(0);
        assert_eq!(accent(&default), Color(0x003d_aee9));
        for (alpha, expected) in [
            (0xff, (0x3d, 0xae, 0xe9, 0xff)),
            (0x38, (0x3d, 0xae, 0xe9, 0x38)),
            (0xdd, (0x3d, 0xae, 0xe9, 0xdd)),
            (0x26, (0x3d, 0xae, 0xe9, 0x26)),
            (0x4d, (0x3d, 0xae, 0xe9, 0x4d)),
            (0x47, (0x3d, 0xae, 0xe9, 0x47)),
            (0xa6, (0x3d, 0xae, 0xe9, 0xa6)),
            (0x33, (0x3d, 0xae, 0xe9, 0x33)),
        ] {
            assert_eq!(accent_rgba(&default, alpha), expected, "alpha {alpha:#x}");
        }
    }

    #[test]
    fn live_accent_drives_every_tint_and_invalid_values_fall_back_to_default() {
        assert_eq!(
            accent_rgba(&with_accent("#EF4048"), 0x38),
            (0xef, 0x40, 0x48, 0x38)
        );
        assert_eq!(accent(&with_accent("teal")), Color(0x001a_bc9c));
        assert_eq!(accent(&with_accent("nonsense")), Color(0x003d_aee9));
    }

    #[test]
    fn shades_follow_the_accent_and_keep_channel_order() {
        let red = shades(Color::FLAME_RED);
        let blue = shades(Color(0x003d_aee9));
        assert_ne!(red.hover, blue.hover);
        assert_ne!(red.pressed, blue.pressed);
        // Pressed is darker than the accent and muted is darker than pressed, per channel.
        assert!(red.pressed.0 >> 16 < 0xef && red.muted.0 >> 16 < red.pressed.0 >> 16);
        // Blue stays blue-dominant after scaling: the blue channel is the largest in pressed.
        assert!((blue.pressed.0 & 0xff) > ((blue.pressed.0 >> 16) & 0xff));
    }
}
