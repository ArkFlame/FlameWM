//! Canonical `FlameWM` product metadata and argv-safe URI validation.

pub const DONATE_URL: &str = "https://paypal.me/LinsaFTW";
pub const SOURCE_URL: &str = "https://github.com/arkflame/flamewm";
pub const WEBSITE_URL: &str = "https://wm.arkflame.com";

#[must_use]
pub fn is_safe_external_uri(uri: &str) -> bool {
    if uri.len() > 2048 || !(uri.starts_with("https://") || uri.starts_with("http://")) {
        return false;
    }
    !uri.chars().any(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '\'' | '"' | '`' | '$' | ';' | '|' | '&' | '<' | '>'
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_product_urls_are_safe() {
        assert!(is_safe_external_uri(DONATE_URL));
        assert!(is_safe_external_uri(SOURCE_URL));
        assert!(is_safe_external_uri(WEBSITE_URL));
    }

    #[test]
    fn shell_metacharacters_are_rejected_even_for_https() {
        assert!(!is_safe_external_uri("https://example.invalid/a;rm"));
    }
}
