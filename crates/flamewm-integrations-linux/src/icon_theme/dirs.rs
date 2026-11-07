//! Icon theme directory metadata from index.theme Directory sections.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconDirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconDir {
    pub size: u32,
    pub scale: u32,
    pub dir_type: IconDirType,
    pub min_size: u32,
    pub max_size: u32,
    pub threshold: u32,
}

impl Default for IconDir {
    fn default() -> Self {
        Self {
            size: 0,
            scale: 1,
            dir_type: IconDirType::Threshold,
            min_size: 0,
            max_size: 0,
            threshold: 2,
        }
    }
}

impl IconDir {
    /// Distance from `requested` to this directory. Lower is better; 0 is exact.
    #[must_use]
    pub fn distance(&self, requested: u32) -> u64 {
        let req = u64::from(requested);
        let nominal = u64::from(self.size) * u64::from(self.scale.max(1));
        match self.dir_type {
            IconDirType::Fixed => req.abs_diff(nominal),
            IconDirType::Scalable => {
                let (mut lo, mut hi) = (u64::from(self.min_size), u64::from(self.max_size));
                if lo == 0 && hi == 0 {
                    return req.abs_diff(nominal);
                }
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                if req < lo {
                    lo - req
                } else {
                    req.saturating_sub(hi)
                }
            }
            IconDirType::Threshold => {
                let t = u64::from(self.threshold);
                if req.abs_diff(nominal) <= t {
                    0
                } else if req < nominal {
                    nominal - t - req
                } else {
                    req - (nominal + t)
                }
            }
        }
    }
}
