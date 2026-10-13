use std::collections::BTreeMap;

use flamewm_api::OutputId;

pub const SCALE_BUCKETS: [u16; 5] = [100, 125, 150, 175, 200];
pub const CACHE_GENERATIONS: u64 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScaleManager {
    scales: BTreeMap<OutputId, u16>,
    cache: BTreeMap<(i32, u16, u64), i32>,
    theme_generation: u64,
}

impl ScaleManager {
    pub fn set_scale(&mut self, output: OutputId, percent: u16) -> bool {
        if !is_supported_scale(percent) {
            return false;
        }
        self.scales.insert(output, percent) != Some(percent)
    }

    #[must_use]
    pub fn scale_for(&self, output: &OutputId) -> u16 {
        self.scales.get(output).copied().unwrap_or(100)
    }

    #[must_use]
    pub fn to_physical(&self, logical: i32, output: &OutputId) -> i32 {
        scale_value(logical, self.scale_for(output))
    }

    pub fn cached_physical_size(&mut self, logical: i32, requested_scale: u16) -> i32 {
        let scale = nearest_supported_scale(requested_scale);
        let key = (logical, scale, self.theme_generation);
        if let Some(value) = self.cache.get(&key) {
            return *value;
        }
        let value = scale_value(logical, scale);
        self.cache.insert(key, value);
        value
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
    matches!(percent, 100 | 125 | 150 | 175 | 200)
}

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
    let scale = i64::from(nearest_supported_scale(percent));
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
    fn cache_is_generation_bounded() {
        let mut manager = ScaleManager::default();
        for generation in 1..=6 {
            manager.set_theme_generation(generation);
            manager.cached_physical_size(18, 100);
        }
        assert_eq!(manager.cache_size(), 3);
    }
}
