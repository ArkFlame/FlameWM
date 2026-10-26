//! Live desktop selection rubber, published on the root window.
//!
//! The rubber band is drawn by the desktop process, below the WM's opaque frame shadows, so
//! the WM cannot see it. The desktop writes this property while a rubber band is live and
//! deletes it when the gesture ends; the WM recomposes only the shadow regions it crosses.

/// Root window property carrying the live rubber band (`CARDINAL[6]`, deleted when idle).
pub const ROOT_RUBBER_PROPERTY: &str = "_FLAMEWM_RUBBER";

/// Border ring width in device pixels, matching the desktop selection material.
pub const ROOT_RUBBER_BORDER_PX: i32 = 2;

/// One live rubber band: root-space rect plus its fill and border colours as straight RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootRubber {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub fill: [u8; 4],
    pub border: [u8; 4],
}

impl RootRubber {
    /// Wire layout: `x`, `y` (two's complement), `width`, `height`, fill RGBA, border RGBA.
    #[must_use]
    pub fn to_cardinals(self) -> [u32; 6] {
        [
            self.x as u32,
            self.y as u32,
            self.width,
            self.height,
            u32::from_be_bytes(self.fill),
            u32::from_be_bytes(self.border),
        ]
    }

    /// Parses the wire layout; `None` for a malformed or empty rect.
    #[must_use]
    pub fn from_cardinals(data: &[u32]) -> Option<Self> {
        let [x, y, width, height, fill, border] = *data else {
            return None;
        };
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return None;
        }
        Some(Self {
            x: x as i32,
            y: y as i32,
            width,
            height,
            fill: fill.to_be_bytes(),
            border: border.to_be_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RootRubber {
        RootRubber {
            x: -40,
            y: 12,
            width: 300,
            height: 200,
            fill: [0xef, 0x40, 0x48, 0x33],
            border: [0xef, 0x40, 0x48, 0xdd],
        }
    }

    #[test]
    fn cardinal_round_trip_keeps_negative_origin_and_colours() {
        let rubber = sample();
        assert_eq!(
            RootRubber::from_cardinals(&rubber.to_cardinals()),
            Some(rubber)
        );
    }

    #[test]
    fn malformed_or_empty_payloads_are_rejected() {
        assert_eq!(RootRubber::from_cardinals(&[]), None);
        assert_eq!(RootRubber::from_cardinals(&[0, 0, 0, 10, 0, 0]), None);
        assert_eq!(RootRubber::from_cardinals(&[0, 0, 10, 0, 0, 0]), None);
        assert_eq!(
            RootRubber::from_cardinals(&[0, 0, u32::MAX, 10, 0, 0]),
            None
        );
    }
}
