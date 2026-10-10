//! Engine-neutral semantic icon resolution policy.

use std::collections::BTreeMap;

use crate::{IconRole, IconTreatment};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSource {
    FlameOverride,
    Breeze,
    SystemTheme,
    PackagedFallback,
    BuiltinGlyph,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconLookup {
    pub value: String,
    pub source: IconSource,
    pub treatment: IconTreatment,
    pub physical_size: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconFallback {
    pub freedesktop_name: &'static str,
    pub fallback_filename: &'static str,
    pub builtin_glyph: &'static str,
}

#[derive(Debug, Clone)]
pub struct IconResolver {
    overrides: BTreeMap<IconRole, String>,
    pub breeze_available: bool,
    pub system_theme_available: bool,
    pub packaged_fallback_available: bool,
    pub theme_generation: u64,
}

impl Default for IconResolver {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            breeze_available: true,
            system_theme_available: true,
            packaged_fallback_available: true,
            theme_generation: 0,
        }
    }
}

impl IconResolver {
    pub fn set_override(&mut self, role: IconRole, path: impl Into<String>) {
        self.overrides.insert(role, path.into());
    }

    pub fn notify_theme_changed(&mut self, generation: u64) {
        self.theme_generation = generation;
    }

    #[must_use]
    pub fn resolve(&self, role: IconRole, scale_percent: u16) -> IconLookup {
        let physical_size = logical_to_physical(logical_size(role), scale_percent);
        if let Some(path) = self.overrides.get(&role) {
            return IconLookup {
                value: path.clone(),
                source: IconSource::FlameOverride,
                treatment: role.treatment(),
                physical_size,
            };
        }
        let fallback = fallback(role);
        if self.breeze_available {
            return lookup(
                format!("breeze:{}", fallback.freedesktop_name),
                IconSource::Breeze,
                role,
                physical_size,
            );
        }
        if self.system_theme_available {
            return lookup(
                format!("system:{}", fallback.freedesktop_name),
                IconSource::SystemTheme,
                role,
                physical_size,
            );
        }
        if self.packaged_fallback_available {
            return lookup(
                format!("fallback:{}", fallback.fallback_filename),
                IconSource::PackagedFallback,
                role,
                physical_size,
            );
        }
        lookup(
            fallback.builtin_glyph.to_owned(),
            IconSource::BuiltinGlyph,
            role,
            physical_size,
        )
    }
}

fn lookup(value: String, source: IconSource, role: IconRole, physical_size: u16) -> IconLookup {
    IconLookup {
        value,
        source,
        treatment: role.treatment(),
        physical_size,
    }
}

#[must_use]
pub const fn logical_size(role: IconRole) -> u16 {
    match role {
        IconRole::Start | IconRole::Taskbar => 32,
        IconRole::Appearance
        | IconRole::Desktop
        | IconRole::Displays
        | IconRole::Fonts
        | IconRole::Hotkeys
        | IconRole::About => 32,
        IconRole::Trash => 16,
        IconRole::NetworkOffline
        | IconRole::NetworkWired
        | IconRole::Wifi0
        | IconRole::Wifi25
        | IconRole::Wifi50
        | IconRole::Wifi75
        | IconRole::Wifi100
        | IconRole::AudioMuted
        | IconRole::AudioLow
        | IconRole::AudioMedium
        | IconRole::AudioHigh
        | IconRole::MediaPrevious
        | IconRole::MediaPlay
        | IconRole::MediaPause
        | IconRole::MediaNext
        | IconRole::Power
        | IconRole::Session => 22,
    }
}

#[must_use]
pub fn logical_to_physical(logical: u16, scale_percent: u16) -> u16 {
    let scale = if matches!(scale_percent, 100 | 125 | 150 | 175 | 200) {
        scale_percent
    } else {
        [100_u16, 125, 150, 175, 200]
            .into_iter()
            .min_by_key(|candidate| candidate.abs_diff(scale_percent))
            .unwrap_or(100)
    };
    u16::try_from((u32::from(logical) * u32::from(scale) + 50) / 100).unwrap_or(u16::MAX)
}

#[must_use]
pub const fn fallback(role: IconRole) -> IconFallback {
    match role {
        IconRole::Start => IconFallback {
            freedesktop_name: "start-here-kde",
            fallback_filename: "flamewm-start.svg",
            builtin_glyph: "[T]",
        },
        IconRole::Taskbar => IconFallback {
            freedesktop_name: "start-here-kde",
            fallback_filename: "flamewm-start.svg",
            builtin_glyph: "[T]",
        },
        IconRole::Appearance => IconFallback {
            freedesktop_name: "preferences-desktop-theme-global",
            fallback_filename: "appearance.svg",
            builtin_glyph: "[A]",
        },
        IconRole::Desktop => IconFallback {
            freedesktop_name: "preferences-desktop",
            fallback_filename: "desktop.svg",
            builtin_glyph: "[D]",
        },
        IconRole::Displays => IconFallback {
            freedesktop_name: "preferences-desktop-display",
            fallback_filename: "displays.svg",
            builtin_glyph: "[M]",
        },
        IconRole::Fonts => IconFallback {
            freedesktop_name: "preferences-desktop-font",
            fallback_filename: "fonts.svg",
            builtin_glyph: "[F]",
        },
        IconRole::Hotkeys => IconFallback {
            freedesktop_name: "preferences-desktop-keyboard",
            fallback_filename: "hotkeys.svg",
            builtin_glyph: "[K]",
        },
        IconRole::About => IconFallback {
            freedesktop_name: "help-about",
            fallback_filename: "about.svg",
            builtin_glyph: "[?]",
        },
        IconRole::NetworkOffline => IconFallback {
            freedesktop_name: "network-offline",
            fallback_filename: "network-offline.svg",
            builtin_glyph: "[N]",
        },
        IconRole::NetworkWired => IconFallback {
            freedesktop_name: "network-wired",
            fallback_filename: "network-wired.svg",
            builtin_glyph: "[N]",
        },
        IconRole::Wifi0
        | IconRole::Wifi25
        | IconRole::Wifi50
        | IconRole::Wifi75
        | IconRole::Wifi100 => IconFallback {
            freedesktop_name: "network-wireless",
            fallback_filename: "network-wireless.svg",
            builtin_glyph: "[W]",
        },
        IconRole::AudioMuted => IconFallback {
            freedesktop_name: "audio-volume-muted",
            fallback_filename: "audio-muted.svg",
            builtin_glyph: "[A]",
        },
        IconRole::AudioLow => IconFallback {
            freedesktop_name: "audio-volume-low",
            fallback_filename: "audio-low.svg",
            builtin_glyph: "[A]",
        },
        IconRole::AudioMedium => IconFallback {
            freedesktop_name: "audio-volume-medium",
            fallback_filename: "audio-medium.svg",
            builtin_glyph: "[A]",
        },
        IconRole::AudioHigh => IconFallback {
            freedesktop_name: "audio-volume-high",
            fallback_filename: "audio-high.svg",
            builtin_glyph: "[A]",
        },
        IconRole::MediaPrevious => IconFallback {
            freedesktop_name: "media-skip-backward",
            fallback_filename: "media-previous.svg",
            builtin_glyph: "[<]",
        },
        IconRole::MediaPlay => IconFallback {
            freedesktop_name: "media-playback-start",
            fallback_filename: "media-play.svg",
            builtin_glyph: "[>]",
        },
        IconRole::MediaPause => IconFallback {
            freedesktop_name: "media-playback-pause",
            fallback_filename: "media-pause.svg",
            builtin_glyph: "[||]",
        },
        IconRole::MediaNext => IconFallback {
            freedesktop_name: "media-skip-forward",
            fallback_filename: "media-next.svg",
            builtin_glyph: "[>]",
        },
        IconRole::Power => IconFallback {
            freedesktop_name: "system-shutdown",
            fallback_filename: "power.svg",
            builtin_glyph: "[P]",
        },
        IconRole::Session => IconFallback {
            freedesktop_name: "system-log-out",
            fallback_filename: "session.svg",
            builtin_glyph: "[S]",
        },
        IconRole::Trash => IconFallback {
            freedesktop_name: "user-trash",
            fallback_filename: "trash.svg",
            builtin_glyph: "[X]",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_prefers_override_before_breeze() {
        let mut resolver = IconResolver::default();
        resolver.set_override(IconRole::Start, "/flame/start.svg");
        assert_eq!(
            resolver.resolve(IconRole::Start, 100).source,
            IconSource::FlameOverride
        );
    }

    #[test]
    fn scale_snaps_to_supported_bucket() {
        assert_eq!(logical_to_physical(32, 149), 48);
    }
}
