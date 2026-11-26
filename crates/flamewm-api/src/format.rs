//! Shared human-readable labels for product surfaces.

/// Byte count as a short label with binary units (B, KiB, MiB, GiB, TiB).
#[must_use]
pub fn human_size(bytes: u64) -> String {
    let mut n = bytes as f64;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB"] {
        if n < 1024.0 {
            break;
        }
        n /= 1024.0;
        unit = candidate;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{n:.1} {unit}")
    }
}

/// Full English month names, January through December.
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Full English weekday names, Sunday through Saturday.
pub const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Full English name for a 1-based month, `"Unknown"` outside `1..=12`.
#[must_use]
pub fn month_name(month: u8) -> &'static str {
    MONTH_NAMES
        .get(usize::from(month.saturating_sub(1)))
        .copied()
        .unwrap_or("Unknown")
}

/// Full English name for a weekday index, where 0 is Sunday and 7 wraps to Sunday.
#[must_use]
pub fn weekday_name(weekday: u8) -> &'static str {
    WEEKDAY_NAMES[usize::from(weekday % 7)]
}

/// Longest prefix of `text` holding at most `max_chars` characters, cut on a char boundary.
#[must_use]
pub fn char_prefix(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => &text[..cut],
        None => text,
    }
}

/// First `keep` characters of `text`, plus `…` when anything was cut.
///
/// The byte-length guard means text that cannot exceed the character budget is copied once and
/// never walked char by char.
#[must_use]
pub fn truncate_chars(text: &str, keep: usize) -> String {
    if text.len() <= keep {
        return text.to_owned();
    }
    let prefix = char_prefix(text, keep);
    if prefix.len() == text.len() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(prefix.len() + '…'.len_utf8());
    out.push_str(prefix);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_uses_binary_units() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(2048), "2.0 KiB");
    }

    #[test]
    fn truncate_chars_keeps_short_text_and_ellipsises_the_rest() {
        assert_eq!(truncate_chars("abc", 8), "abc");
        assert_eq!(truncate_chars("abcdefghij", 3), "abc…");
        assert_eq!(truncate_chars("éééé", 2), "éé…");
        assert_eq!(char_prefix("éééé", 2), "éé");
    }

    #[test]
    fn month_and_weekday_names_index_from_one_and_zero() {
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(13), "Unknown");
        assert_eq!(weekday_name(0), "Sunday");
        assert_eq!(weekday_name(7), "Sunday");
    }
}
