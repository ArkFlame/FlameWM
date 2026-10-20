//! Persistent two-monitor left-to-right preference. Contains connector names only.
//! No X11 dependency, no KDE config modification, no saved display modes.
use std::path::PathBuf;

pub fn preference_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(root.join("flamewm").join("display-order"))
}

/// Connector names originate in the physical RandR snapshot. Refuse whitespace
/// and punctuation that could make future parsers interpret more than one name.
pub fn connector_safe(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn parse_order(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let left = lines.next()?.strip_prefix("left=")?;
    let right = lines.next()?.strip_prefix("right=")?;
    if lines.next().is_some() || !connector_safe(left) || !connector_safe(right) || left == right {
        return None;
    }
    Some((left.to_owned(), right.to_owned()))
}

pub fn load_order() -> Option<(String, String)> {
    parse_order(&std::fs::read_to_string(preference_path()?).ok()?)
}

pub fn save_order(left: &str, right: &str) -> Result<(), String> {
    if !connector_safe(left) || !connector_safe(right) || left == right {
        return Err("invalid RandR connector preference".to_owned());
    }
    let dest = preference_path().ok_or("XDG config directory unavailable")?;
    let parent = dest.parent().ok_or("invalid config directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = dest.with_extension(format!("tmp-{}", std::process::id()));
    // Atomic file replacement; do not follow an existing destination symlink.
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&temp).map_err(|e| e.to_string())?;
    if let Err(e) = file.write_all(format!("left={left}\nright={right}\n").as_bytes()) {
        let _ = std::fs::remove_file(&temp); return Err(e.to_string());
    }
    if let Err(e) = std::fs::rename(&temp, &dest) {
        let _ = std::fs::remove_file(&temp); return Err(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validated_connector_records() {
        assert_eq!(parse_order("left=DP-1\nright=HDMI-1\n"), Some(("DP-1".into(), "HDMI-1".into())));
        for invalid in ["left=HDMI-1\nright=HDMI-1\n", "left=x;rm\nright=DP-1\n", "left=A\nright=B\nextra=x", "left=A\n"] {
            assert!(parse_order(invalid).is_none());
        }
    }
}
