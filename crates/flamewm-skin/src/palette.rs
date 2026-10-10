//! Exact colors from `examples/flamewm-v8/flamewm.css`, line 1 and component rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlphaColor {
    pub color: Rgb,
    pub alpha: u8,
}

pub const ACCENT: Rgb = Rgb(0xef4048);
pub const DESKTOP: Rgb = Rgb(0x000000);
pub const PANEL: Rgb = Rgb(0x1b1e20);
pub const SURFACE: Rgb = Rgb(0x202326);
pub const SIDEBAR: Rgb = Rgb(0x1b1e20);
pub const TEXT: Rgb = Rgb(0xf1f2f3);
pub const MUTED: Rgb = Rgb(0xaeb4bb);
pub const STICKY_BACKGROUND: Rgb = Rgb(0xffe56b);
pub const STICKY_TEXT: Rgb = Rgb(0x171717);

pub const ACCENT_SOFT: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x38,
};
pub const DESKTOP_HOVER: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x26,
};
pub const DESKTOP_SELECTED: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x47,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Rgb,
    pub desktop: Rgb,
    pub panel: Rgb,
    pub surface: Rgb,
    pub sidebar: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub sticky_background: Rgb,
    pub sticky_text: Rgb,
}

impl Palette {
    pub const RWR_0_0_9: Self = Self {
        accent: ACCENT,
        desktop: DESKTOP,
        panel: PANEL,
        surface: SURFACE,
        sidebar: SIDEBAR,
        text: TEXT,
        muted: MUTED,
        sticky_background: STICKY_BACKGROUND,
        sticky_text: STICKY_TEXT,
    };
}

impl Default for Palette {
    fn default() -> Self {
        Self::RWR_0_0_9
    }
}
