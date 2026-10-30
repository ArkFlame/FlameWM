use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
pub struct FxHasher(u64);

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.add(u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8])));
        }
        for &byte in chunks.remainder() {
            self.add(u64::from(byte));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> FxHasher {
        FxHasher::default()
    }
}

pub type FastMap<K, V> = HashMap<K, V, FxBuildHasher>;
pub type FastSet<K> = HashSet<K, FxBuildHasher>;

#[cfg(test)]
mod tests {
    use super::FxBuildHasher;
    use std::hash::BuildHasher;

    #[test]
    fn sequential_keys_spread_into_high_bits() {
        let tags: std::collections::HashSet<u64> = (0u32..256)
            .map(|key| FxBuildHasher.hash_one(key) >> 57)
            .collect();
        assert!(tags.len() > 64, "only {} distinct control tags", tags.len());
    }
}
