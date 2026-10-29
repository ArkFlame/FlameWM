//! Display strings for sizes, ages and paths. Pure functions of their inputs.

/// Binary-unit size such as `512 B`, `1.5 KB`, `230 MB`.
#[must_use]
pub fn bytes(value: u64) -> String {
    if value == 0 {
        return "0 B".to_owned();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    if scaled >= 100.0 || unit == 0 {
        format!("{:.0} {}", scaled, UNITS[unit])
    } else {
        format!("{:.1} {}", scaled, UNITS[unit])
    }
}

/// Age relative to `now`, in the same steps as the reference: `just now`, `12m ago`, `3h ago`,
/// `2d ago`, `5mo ago`, then the calendar date.
#[must_use]
pub fn relative_time(modified: i64, now: i64) -> String {
    if modified <= 0 {
        return String::new();
    }
    let diff = now.saturating_sub(modified).max(0);
    if diff < 60 {
        "just now".to_owned()
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else if diff < 86_400 * 365 {
        format!("{}mo ago", diff / (86_400 * 30))
    } else {
        let (year, month, day) = civil_date(modified / 86_400);
        format!("{year:04}-{month:02}-{day:02}")
    }
}

/// Calendar date for a count of days since 1970-01-01 (proleptic Gregorian).
#[must_use]
pub fn civil_date(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Parent folder for display: the home folder collapses to `~`.
#[must_use]
pub fn parent_for_display(path: &str, home: &str) -> String {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let home = home.trim_end_matches('/');
    if !home.is_empty() && (parent == home || parent.starts_with(&format!("{home}/"))) {
        return format!("~{}", &parent[home.len()..]);
    }
    if parent.is_empty() {
        "/".to_owned()
    } else {
        parent.to_owned()
    }
}

/// Thousands-separated count such as `12,345`.
#[must_use]
pub fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_binary_units_and_precision_rules() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(230 * 1024 * 1024), "230 MB");
    }

    #[test]
    fn relative_time_steps() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 720, now), "12m ago");
        assert_eq!(relative_time(now - 3 * 3_600, now), "3h ago");
        assert_eq!(relative_time(now - 2 * 86_400, now), "2d ago");
        assert_eq!(relative_time(0, now), "");
    }

    #[test]
    fn civil_date_matches_known_days() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(19_723), (2024, 1, 1));
        assert_eq!(civil_date(11_016), (2000, 2, 29));
    }

    #[test]
    fn parent_collapses_home() {
        assert_eq!(
            parent_for_display("/home/u/Work/a.txt", "/home/u"),
            "~/Work"
        );
        assert_eq!(parent_for_display("/home/u/a.txt", "/home/u"), "~");
        assert_eq!(parent_for_display("/srv/a.txt", "/home/u"), "/srv");
        assert_eq!(parent_for_display("/home/ux/a.txt", "/home/u"), "/home/ux");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(7), "7");
        assert_eq!(count(12_345), "12,345");
        assert_eq!(count(1_000_000), "1,000,000");
    }
}
