//! Typography values promoted from the CSS and HTML prototype.

use crate::palette::{MUTED, Rgb, TEXT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWeight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub size_px: u16,
    pub weight: FontWeight,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typography {
    pub family: &'static str,
    pub body: TextStyle,
    pub desktop_label: TextStyle,
    pub title: TextStyle,
    pub page_title: TextStyle,
    pub sticky_title: TextStyle,
    pub muted: TextStyle,
}

impl Typography {
    /// Values correspond to body, desktop-label, window-title, page-title, sticky-title and muted CSS rules.
    pub const RWR_0_0_9: Self = Self {
        family: "IBM Plex Sans",
        body: TextStyle {
            size_px: 13,
            weight: FontWeight::Regular,
            color: TEXT,
        },
        desktop_label: TextStyle {
            size_px: 12,
            weight: FontWeight::Regular,
            color: TEXT,
        },
        title: TextStyle {
            size_px: 12,
            weight: FontWeight::Semibold,
            color: TEXT,
        },
        page_title: TextStyle {
            size_px: 21,
            weight: FontWeight::Medium,
            color: TEXT,
        },
        sticky_title: TextStyle {
            size_px: 15,
            weight: FontWeight::Bold,
            color: Rgb(0x0017_1717),
        },
        muted: TextStyle {
            size_px: 13,
            weight: FontWeight::Regular,
            color: MUTED,
        },
    };
}

impl Default for Typography {
    fn default() -> Self {
        Self::RWR_0_0_9
    }
}
