use std::collections::BTreeMap;

use flamewm_api::{ErrorCode, FlameError, FlameResult, OutputId, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeMetrics {
    pub titlebar_height: i32,
    pub border_x: i32,
    pub border_y: i32,
    pub corner_size_x: i32,
    pub corner_size_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowChrome {
    pub decorated: bool,
    pub titlebar: bool,
    pub border: bool,
    pub minimize_button: bool,
    pub maximize_button: bool,
    pub close_button: bool,
    pub resizable: bool,
    pub rollover_buttons: bool,
}

impl Default for WindowChrome {
    fn default() -> Self {
        Self {
            decorated: true,
            titlebar: true,
            border: true,
            minimize_button: true,
            maximize_button: true,
            close_button: true,
            resizable: true,
            rollover_buttons: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromePolicy {
    revision: u64,
    scales: BTreeMap<OutputId, u16>,
    hide_title_when_maximized: bool,
    hide_decor_when_maximized: bool,
    accent: String,
}

impl Default for ChromePolicy {
    fn default() -> Self {
        Self {
            revision: 1,
            scales: BTreeMap::new(),
            hide_title_when_maximized: false,
            hide_decor_when_maximized: false,
            accent: "#EF4048".to_owned(),
        }
    }
}

impl ChromePolicy {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn accent(&self) -> &str {
        &self.accent
    }

    pub fn set_accent(&mut self, value: &str) -> FlameResult<()> {
        if parse_hex_rgb(value).is_none() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "accent must be #RRGGBB",
            ));
        }
        if self.accent != value {
            self.accent = value.to_owned();
            self.revision = self.revision.saturating_add(1);
        }
        Ok(())
    }

    #[must_use]
    pub fn scale_for(&self, output: &OutputId) -> u16 {
        self.scales.get(output).copied().unwrap_or(100)
    }

    #[must_use]
    pub fn metrics_for(&self, output: &OutputId) -> ChromeMetrics {
        let scale = i32::from(self.scale_for(output));
        ChromeMetrics {
            titlebar_height: (32 * scale + 50) / 100,
            border_x: ((scale + 50) / 100).max(1),
            border_y: ((scale + 50) / 100).max(1),
            corner_size_x: (8 * scale + 50) / 100,
            corner_size_y: (8 * scale + 50) / 100,
        }
    }

    #[must_use]
    pub fn centered_title_rect(
        titlebar: Rect,
        title_width: i32,
        buttons_left_width: i32,
        buttons_right_width: i32,
    ) -> Rect {
        if titlebar.width <= 0 || titlebar.height <= 0 {
            return Rect::new(titlebar.x, titlebar.y, 0, titlebar.height.max(0));
        }
        let left = titlebar.x + buttons_left_width.max(0);
        let right = titlebar.right() - buttons_right_width.max(0);
        let available = right - left;
        if available <= 0 || title_width <= 0 {
            return Rect::new(left, titlebar.y, 0, titlebar.height);
        }
        let width = title_width.min(available);
        let ideal = titlebar.x + (titlebar.width - width) / 2;
        let x = ideal.clamp(left, right - width);
        Rect::new(x, titlebar.y, width, titlebar.height)
    }
}

/// Alpha of the derived `--accent-soft` token at full `accentOpacity`.
pub const ACCENT_SOFT_ALPHA: u8 = 0x38;

/// Strict `#RRGGBB`: `#` then exactly six ASCII hex digits. Signs, `0x`,
/// and non-ASCII input are rejected.
#[must_use]
pub fn parse_hex_rgb(value: &str) -> Option<(u8, u8, u8)> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(digits, 16).ok()?;
    Some(((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_frame_centered_then_collision_clamped() {
        let titlebar = Rect::new(100, 20, 500, 32);
        assert_eq!(
            ChromePolicy::centered_title_rect(titlebar, 100, 30, 126),
            Rect::new(300, 20, 100, 32)
        );
        assert_eq!(
            ChromePolicy::centered_title_rect(titlebar, 420, 30, 126),
            Rect::new(130, 20, 344, 32)
        );
    }

    #[test]
    fn hex_rgb_parser_is_strict_and_accent_rejects_sign_forms() {
        assert_eq!(parse_hex_rgb("#123456"), Some((0x12, 0x34, 0x56)));
        assert_eq!(parse_hex_rgb("#+12345"), None);
        assert_eq!(parse_hex_rgb("#-12345"), None);
        assert_eq!(parse_hex_rgb("#0x1234"), None);
        assert_eq!(parse_hex_rgb("123456"), None);
        assert_eq!(parse_hex_rgb("#é1234"), None);
        let mut policy = ChromePolicy::default();
        assert!(policy.set_accent("#+12345").is_err());
        assert!(policy.set_accent("#00ff00").is_ok());
    }

    #[test]
    fn default_corner_metrics_match_current_native_theme() {
        let metrics = ChromePolicy::default().metrics_for(&OutputId::new("eDP-1"));
        assert_eq!((metrics.corner_size_x, metrics.corner_size_y), (8, 8));
    }
}
