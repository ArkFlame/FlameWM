//! ARK AI provider: an OpenAI-compatible endpoint that needs an API key.

use crate::openai::Endpoint;

/// OpenAI-compatible base URL. Paths are `/models` and `/chat/completions` under it.
pub const ARK_BASE_URL: &str = "https://ai.arkflame.com/v1";
/// Public default key, shipped to every install. Settings can replace it.
pub const DEFAULT_API_KEY: &str = "ark_23bR3ryTRW5t9u8R-XAX6dYmtnJ33zifmUl-_ovjcok";
pub const DEFAULT_MODEL: &str = "flash";
/// Used when `GET /models` cannot be read.
pub const FALLBACK_MODELS: [&str; 3] = ["lite", "flash", "pro"];
/// Test hook: a loopback base URL that replaces `ARK_BASE_URL`. Other hosts are ignored.
pub const BASE_URL_ENV: &str = "FLAMEWM_ASSISTANT_ARK_BASE";
pub const LABEL: &str = "ARK AI";

/// The ARK base URL in use: the loopback test hook when set, otherwise `ARK_BASE_URL`.
pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| {
            value.starts_with("http://127.0.0.1") || value.starts_with("http://localhost")
        })
        .unwrap_or_else(|| ARK_BASE_URL.to_owned())
}

/// Endpoint for ARK AI with the given key.
pub fn endpoint(api_key: &str) -> Endpoint {
    Endpoint {
        label: LABEL,
        base_url: base_url(),
        bearer: api_key.to_owned(),
        max_time: None,
    }
}

/// Checks a key before it is stored or sent: non-empty, no spaces, `ark_` prefix.
pub fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("Enter an API key, or press Reset to use the default key.".into());
    }
    if key.chars().any(char::is_whitespace) {
        return Err("The API key cannot contain spaces.".into());
    }
    if !key.starts_with("ark_") {
        return Err("The API key must start with ark_.".into());
    }
    Ok(())
}

/// The key to use: the stored one when it is set, otherwise the public default.
pub fn key_or_default(stored: Option<&str>) -> String {
    match stored.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => key.to_owned(),
        None => DEFAULT_API_KEY.to_owned(),
    }
}

/// Settings display: the first eight characters and dots. Never the whole key.
pub fn masked_key(key: &str) -> String {
    let shown: String = key.chars().take(8).collect();
    format!("{shown}{}", "\u{2022}".repeat(12))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_rules_default_and_mask() {
        assert!(validate_key(DEFAULT_API_KEY).is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("ark_has space").is_err());
        assert!(validate_key("sk-123").is_err());
        assert_eq!(key_or_default(None), DEFAULT_API_KEY);
        assert_eq!(key_or_default(Some("  ")), DEFAULT_API_KEY);
        assert_eq!(key_or_default(Some("ark_mine")), "ark_mine");
        assert_eq!(
            masked_key(DEFAULT_API_KEY),
            format!("ark_23bR{}", "\u{2022}".repeat(12))
        );
    }

    #[test]
    fn endpoint_carries_key_as_bearer_and_label() {
        let ep = endpoint("ark_k");
        assert_eq!(ep.bearer, "ark_k");
        assert_eq!(ep.label, LABEL);
        assert_eq!(ep.max_time, None);
    }
}
