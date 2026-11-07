//! Strict `#RRGGBB` colour text parsing. Every product layer parses colour settings through here.

/// Parses `#RRGGBB`. The `#` is required; signs, `0x` and non-ASCII digits are rejected.
#[must_use]
pub fn parse_hex_rgb(value: &str) -> Option<(u8, u8, u8)> {
    let rgb = parse_digits(value.strip_prefix('#')?)?;
    Some(((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

/// Parses `#RRGGBB` or bare `RRGGBB` into `0xRRGGBB`.
#[must_use]
pub fn parse_hex_u32(value: &str) -> Option<u32> {
    parse_digits(value.strip_prefix('#').unwrap_or(value))
}

fn parse_digits(digits: &str) -> Option<u32> {
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_requires_hash_and_splits_channels() {
        assert_eq!(parse_hex_rgb("#123456"), Some((0x12, 0x34, 0x56)));
        assert_eq!(parse_hex_rgb("123456"), None);
        assert_eq!(parse_hex_rgb("#+12345"), None);
        assert_eq!(parse_hex_rgb("#0x1234"), None);
        assert_eq!(parse_hex_rgb("#é1234"), None);
    }

    #[test]
    fn u32_accepts_optional_hash() {
        assert_eq!(parse_hex_u32("#1E1E1E"), Some(0x001e_1e1e));
        assert_eq!(parse_hex_u32("ff00aa"), Some(0x00ff_00aa));
        assert_eq!(parse_hex_u32("#12345"), None);
        assert_eq!(parse_hex_u32("#zzzzzz"), None);
    }
}
