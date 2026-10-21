use std::collections::BTreeMap;

use flamewm_api::OutputId;

pub const SCALE_BUCKETS: [u16; 21] = [
    100, 105, 110, 115, 120, 125, 130, 135, 140, 145, 150, 155, 160, 165, 170, 175, 180, 185, 190,
    195, 200,
];
pub const CACHE_GENERATIONS: u64 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScaleManager {
    scales: BTreeMap<OutputId, u16>,
    cache: BTreeMap<(i32, u16, u64), i32>,
    theme_generation: u64,
}

impl ScaleManager {
    pub fn cached_physical_size(&mut self, logical: i32, requested_scale: u16) -> i32 {
        let scale = effective_scale(requested_scale);
        *self
            .cache
            .entry((logical, scale, self.theme_generation))
            .or_insert_with(|| scale_value(logical, scale))
    }

    pub fn set_theme_generation(&mut self, generation: u64) {
        self.theme_generation = generation;
        self.retire_old_generations();
    }

    #[must_use]
    pub const fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    #[cfg(test)]
    #[must_use]
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    fn retire_old_generations(&mut self) {
        let oldest = self.theme_generation.saturating_sub(CACHE_GENERATIONS - 1);
        self.cache.retain(|(_, _, generation), _| {
            *generation >= oldest && *generation <= self.theme_generation
        });
    }
}

#[must_use]
pub const fn is_supported_scale(percent: u16) -> bool {
    percent >= 100 && percent <= 200 && percent % 5 == 0
}

#[must_use]
pub fn effective_scale(percent: u16) -> u16 {
    if is_supported_scale(percent) {
        percent
    } else {
        100
    }
}

/// Rounds only for non-authoritative visual previews; never use for settings writes.
#[must_use]
pub fn nearest_supported_scale(percent: u16) -> u16 {
    SCALE_BUCKETS
        .iter()
        .copied()
        .min_by_key(|bucket| bucket.abs_diff(percent))
        .unwrap_or(100)
}

#[must_use]
pub fn scale_value(logical: i32, percent: u16) -> i32 {
    let scale = i64::from(effective_scale(percent));
    let scaled = (i64::from(logical) * scale + if logical >= 0 { 50 } else { -50 }) / 100;
    scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_scale_rounds_logical_size() {
        assert_eq!(scale_value(44, 125), 55);
        assert_eq!(scale_value(44, 150), 66);
    }

    #[test]
    fn all_five_percent_steps_are_exact_and_cached_independently() {
        let mut manager = ScaleManager::default();
        for percent in SCALE_BUCKETS {
            assert!(is_supported_scale(percent));
            assert_eq!(nearest_supported_scale(percent), percent);
            assert_eq!(scale_value(100, percent), i32::from(percent));
            assert_eq!(
                manager.cached_physical_size(100, percent),
                i32::from(percent)
            );
        }
        assert_eq!(manager.cache_size(), 21);
        for percent in [0, 99, 101, 104, 106, 133, 199, 201, u16::MAX] {
            assert!(!is_supported_scale(percent));
            assert_eq!(effective_scale(percent), 100);
        }
        assert_eq!(scale_value(44, 100), 44);
        assert_eq!(scale_value(100, 135), 135);
        assert_eq!(scale_value(100, 195), 195);
    }

    #[test]
    fn cache_is_generation_bounded() {
        let mut manager = ScaleManager::default();
        for generation in 1..=6 {
            manager.set_theme_generation(generation);
            manager.cached_physical_size(18, 100);
        }
        assert_eq!(manager.cache_size(), 3);
    }
}
