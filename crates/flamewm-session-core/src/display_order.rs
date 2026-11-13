//! Persistent left-to-right monitor order. Contains connector names only.
//! No X11 dependency, no KDE config modification, no saved display modes.
//!
//! Format v2 is one line, `order=A,B,C`. Format v1 (`left=A` / `right=B`, two
//! outputs) still reads as `[A, B]`, so a preference saved before v2 keeps working.
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Upper bound on remembered outputs, so a corrupt file cannot make restore unbounded.
pub const MAX_OUTPUTS: usize = 16;

pub fn preference_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(root.join("flamewm").join("display-order"))
}

/// Connector names originate in the physical RandR snapshot. Refuse whitespace
/// and punctuation that could make future parsers interpret more than one name.
pub fn connector_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn order_is_valid(names: &[String]) -> bool {
    (2..=MAX_OUTPUTS).contains(&names.len())
        && names.iter().all(|name| connector_safe(name))
        && names.iter().collect::<BTreeSet<_>>().len() == names.len()
}

pub fn parse_order(text: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let names: Vec<String> = if let Some(list) = first.strip_prefix("order=") {
        if lines.next().is_some() {
            return None;
        }
        list.split(',').map(str::to_owned).collect()
    } else {
        // v1 two-output format, kept readable for upgrades.
        let left = first.strip_prefix("left=")?;
        let right = lines.next()?.strip_prefix("right=")?;
        if lines.next().is_some() {
            return None;
        }
        vec![left.to_owned(), right.to_owned()]
    };
    order_is_valid(&names).then_some(names)
}

pub fn load_order() -> Option<Vec<String>> {
    parse_order(&std::fs::read_to_string(preference_path()?).ok()?)
}

/// Saves `order` (left to right). Always writes v2.
pub fn save_order(order: &[String]) -> Result<(), String> {
    if !order_is_valid(order) {
        return Err("invalid RandR connector preference".to_owned());
    }
    let dest = preference_path().ok_or("XDG config directory unavailable")?;
    let parent = dest.parent().ok_or("invalid config directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = dest.with_extension(format!("tmp-{}", std::process::id()));
    // Atomic file replacement; do not follow an existing destination symlink.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    if let Err(e) = file.write_all(format!("order={}\n", order.join(",")).as_bytes()) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    if let Err(e) = std::fs::rename(&temp, &dest) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn v1_two_output_records_still_read() {
        assert_eq!(
            parse_order("left=DP-1\nright=HDMI-1\n"),
            Some(names(&["DP-1", "HDMI-1"]))
        );
    }

    #[test]
    fn v2_ordered_list_round_trips_and_keeps_order() {
        assert_eq!(
            parse_order("order=DP-1,HDMI-1,eDP-1\n"),
            Some(names(&["DP-1", "HDMI-1", "eDP-1"]))
        );
    }

    #[test]
    fn invalid_records_are_rejected() {
        for invalid in [
            "left=HDMI-1\nright=HDMI-1\n",
            "left=x;rm\nright=DP-1\n",
            "left=A\nright=B\nextra=x",
            "left=A\n",
            "order=A\n",
            "order=A,A\n",
            "order=A,,B\n",
            "order=A,B\nextra=x",
            "order=A B,C\n",
        ] {
            assert!(parse_order(invalid).is_none(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn oversized_order_is_rejected() {
        let many: Vec<String> = (0..=MAX_OUTPUTS).map(|i| format!("OUT-{i}")).collect();
        assert!(!order_is_valid(&many));
        assert!(order_is_valid(&many[..MAX_OUTPUTS]));
    }
}
