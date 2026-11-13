//! Motion policy shared by every decorative-animation owner (WM, shell, settings).
//!
//! Precedent: KDE Plasma "Animation speed" (drag to instant), GNOME `enable-animations`, and the
//! CSS `prefers-reduced-motion` media query. Reduced motion removes decorative motion only; the
//! end state of every change still applies at once.

use crate::settings::{SettingsSnapshot, setting_bool, setting_integer};

/// `animationSpeed` default in hundredths (100 = 1.0x).
pub const DEFAULT_SPEED: i64 = 100;
/// `animationSpeed` bounds in hundredths.
pub const MIN_SPEED: i64 = 25;
pub const MAX_SPEED: i64 = 200;

/// The one motion policy. Returns the animation speed in hundredths (25..=200), or `0` when
/// `reducedMotion` is on: `0` means every decorative transition is skipped and its end state
/// applied immediately. Callers never read `reducedMotion` or `animationSpeed` themselves.
#[must_use]
pub fn motion_scale(snapshot: &SettingsSnapshot) -> u16 {
    if setting_bool(snapshot, "reducedMotion", false) {
        return 0;
    }
    setting_integer(snapshot, "animationSpeed", DEFAULT_SPEED).clamp(MIN_SPEED, MAX_SPEED) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingValue;

    fn snapshot(reduced: Option<bool>, speed: Option<i64>) -> SettingsSnapshot {
        let mut s = SettingsSnapshot::new(1);
        if let Some(value) = reduced {
            s.values
                .insert("reducedMotion".into(), SettingValue::Boolean(value));
        }
        if let Some(value) = speed {
            s.values
                .insert("animationSpeed".into(), SettingValue::Integer(value));
        }
        s
    }

    #[test]
    fn default_is_full_speed() {
        assert_eq!(motion_scale(&snapshot(None, None)), 100);
        assert_eq!(motion_scale(&snapshot(Some(false), Some(50))), 50);
    }

    #[test]
    fn reduced_motion_is_instant_whatever_the_speed() {
        assert_eq!(motion_scale(&snapshot(Some(true), None)), 0);
        assert_eq!(motion_scale(&snapshot(Some(true), Some(200))), 0);
    }

    #[test]
    fn speed_clamps_into_product_bounds() {
        assert_eq!(motion_scale(&snapshot(None, Some(1000))), 200);
        assert_eq!(motion_scale(&snapshot(None, Some(0))), 25);
    }
}
