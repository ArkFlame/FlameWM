//! Types for a future gamma backend; these do not persist or apply settings.

/// The canonical schedule uses local minutes after midnight, or the weather location's sun times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightLightSchedule {
    Manual {
        start: NightLightMinute,
        end: NightLightMinute,
    },
    SunsetToSunrise,
}

/// A local time of day, measured in minutes after midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightLightMinute(u16);

impl NightLightMinute {
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl NightLightSchedule {
    /// Construct a manual schedule, rejecting times outside a local day.
    #[must_use]
    pub const fn manual(start_minute: u16, end_minute: u16) -> Option<Self> {
        if start_minute < 1440 && end_minute < 1440 {
            Some(Self::Manual {
                start: NightLightMinute(start_minute),
                end: NightLightMinute(end_minute),
            })
        } else {
            None
        }
    }
}

/// Color temperature in kelvin. This is not interchangeable with legacy filter intensity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightLightTemperature(u16);

impl NightLightTemperature {
    #[must_use]
    pub const fn new(kelvin: u16) -> Option<Self> {
        if kelvin >= 1000 && kelvin <= 6500 {
            Some(Self(kelvin))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn kelvin(self) -> u16 {
        self.0
    }
}

/// A transient preview request, not an authoritative or persisted setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightLightPreview {
    Off,
    On(NightLightTemperature),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_temperature_and_manual_time_boundaries() {
        for value in [1000, 6500] {
            assert_eq!(NightLightTemperature::new(value).unwrap().kelvin(), value);
        }
        for value in [0, 999, 6501, u16::MAX] {
            assert_eq!(NightLightTemperature::new(value), None);
        }
        assert!(NightLightSchedule::manual(1439, 0).is_some());
        assert!(NightLightSchedule::manual(1440, 0).is_none());
        assert!(NightLightSchedule::manual(0, 1440).is_none());
    }
}
