//! Last-known `FlameWM` settings snapshot, kept under `$XDG_CACHE_HOME/flamewm/terminal` so the first paint can
//! use it before `GetSettings` answers. A plain line format (no serde): one header line, then one line per value.
//! Text values are hex-encoded, so keys and values never need escaping. Writes go to a temp file and rename into
//! place, so a reader sees either the old or the new snapshot. Every failure is non-fatal: the cache is a hint.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

const HEADER: &str = "flamewm-settings-cache 1";

/// Cache file path: `$XDG_CACHE_HOME/flamewm/terminal/settings.snapshot`, else `~/.cache/...`.
#[must_use]
pub fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("flamewm")
        .join("terminal")
        .join("settings.snapshot")
}

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// Serialises a snapshot to the cache format.
#[must_use]
pub fn encode(snapshot: &SettingsSnapshot) -> String {
    let mut out = format!("{HEADER}\nrevision {}\n", snapshot.revision);
    for (key, value) in &snapshot.values {
        let line = match value {
            SettingValue::Boolean(value) => format!("bool {} {}", hex(key), u8::from(*value)),
            SettingValue::Integer(value) => format!("int {} {value}", hex(key)),
            SettingValue::Percent(value) => format!("pct {} {value}", hex(key)),
            SettingValue::Rgb(value) => format!("rgb {} {value}", hex(key)),
            SettingValue::Text(value) => format!("text {} {}", hex(key), hex(value)),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Parses the cache format. `None` for any malformed or unknown content, so a damaged file is ignored.
#[must_use]
pub fn decode(text: &str) -> Option<SettingsSnapshot> {
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let revision = lines.next()?.strip_prefix("revision ")?.parse().ok()?;
    let mut snapshot = SettingsSnapshot::new(revision);
    for line in lines {
        let mut fields = line.split(' ');
        let kind = fields.next()?;
        let key = unhex(fields.next()?)?;
        let raw = fields.next()?;
        if fields.next().is_some() {
            return None;
        }
        let value = match kind {
            "bool" => SettingValue::Boolean(raw == "1"),
            "int" => SettingValue::Integer(raw.parse().ok()?),
            "pct" => SettingValue::Percent(raw.parse().ok()?),
            "rgb" => SettingValue::Rgb(raw.parse().ok()?),
            "text" => SettingValue::Text(unhex(raw)?),
            _ => return None,
        };
        snapshot.values.insert(key, value);
    }
    Some(snapshot)
}

/// Reads the cached snapshot, or `None` when there is none or it cannot be parsed.
#[must_use]
pub fn load(path: &Path) -> Option<SettingsSnapshot> {
    decode(&fs::read_to_string(path).ok()?)
}

/// Writes the snapshot atomically (temp file in the same directory, then rename). Errors are ignored by callers.
pub fn store(path: &Path, snapshot: &SettingsSnapshot) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".settings.snapshot.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(encode(snapshot).as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_every_value_kind() {
        let mut snapshot = SettingsSnapshot::new(42);
        snapshot.values.insert(
            "wallpaper".into(),
            SettingValue::Text("/a b\tc\n/é.png".into()),
        );
        snapshot
            .values
            .insert("surfaceBlurPercent".into(), SettingValue::Percent(80));
        snapshot
            .values
            .insert("accent".into(), SettingValue::Rgb(0x003D_AEE9));
        snapshot
            .values
            .insert("delta".into(), SettingValue::Integer(-7));
        snapshot
            .values
            .insert("flag".into(), SettingValue::Boolean(true));
        assert_eq!(decode(&encode(&snapshot)), Some(snapshot));
    }

    #[test]
    fn damaged_or_foreign_files_are_ignored() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("other 1\nrevision 1\n"), None);
        assert_eq!(
            decode("flamewm-settings-cache 1\nrevision 1\nbogus 00 1\n"),
            None
        );
        assert_eq!(
            decode("flamewm-settings-cache 1\nrevision 1\npct 6163 999\n"),
            None
        );
        assert_eq!(
            decode("flamewm-settings-cache 1\nrevision 1\ntext 6162 zz\n"),
            None
        );
    }

    #[test]
    fn store_then_load_reads_back_and_replaces_in_place() {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-settings-cache-test-{}",
            std::process::id()
        ));
        let path = dir.join("settings.snapshot");
        let mut first = SettingsSnapshot::new(1);
        first.values.insert("accent".into(), SettingValue::Rgb(1));
        store(&path, &first).unwrap();
        assert_eq!(load(&path), Some(first));
        let second = SettingsSnapshot::new(2);
        store(&path, &second).unwrap();
        assert_eq!(load(&path), Some(second));
        let _ = fs::remove_dir_all(&dir);
    }
}
