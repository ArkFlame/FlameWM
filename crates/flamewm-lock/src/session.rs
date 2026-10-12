//! Typed settings snapshot and user identity for the lock screen.
use std::collections::HashMap;
use std::time::Duration;

use flamewm_api::settings::SettingValue;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;

use crate::settings::LockSettings;

const SETTINGS_TIMEOUT: Duration = Duration::from_secs(2);

/// Loads lock settings from the control plane. Any failure yields defaults so
/// the lock screen still works without the settings service.
pub fn load_settings() -> LockSettings {
    let values: HashMap<String, String> = fetch_snapshot()
        .map(|snapshot| {
            snapshot
                .values
                .iter()
                .map(|(key, value)| (key.clone(), value_text(value)))
                .collect()
        })
        .unwrap_or_default();
    LockSettings::from_lookup(|key| values.get(key).map(String::as_str))
}

fn fetch_snapshot() -> Option<flamewm_api::settings::SettingsSnapshot> {
    let client = ControlClient::connect(BusKind::Session).ok()?;
    match client
        .call_with_timeout(&ControlRequest::GetSettings, SETTINGS_TIMEOUT)
        .ok()?
    {
        ControlResponse::Settings(snapshot) => Some(snapshot),
        _ => None,
    }
}

fn value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Boolean(flag) => flag.to_string(),
        SettingValue::Integer(number) => number.to_string(),
        SettingValue::Percent(percent) => percent.to_string(),
        SettingValue::Rgb(rgb) => format!("#{rgb:06x}"),
        SettingValue::Text(text) => text.clone(),
    }
}

/// Name of the account owning `uid` in passwd-formatted `text` (`name:x:uid:gid:...`).
fn name_for_uid(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        let id: u32 = fields.next()?.parse().ok()?;
        (id == uid && !name.is_empty()).then(|| name.to_string())
    })
}

/// Real uid of this process from `/proc/self/status` (`Uid:\t<real>\t<effective>...`).
fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// The account being unlocked. The process's real uid is authoritative (the environment is caller
/// controlled); `USER`/`LOGNAME` are used only when the uid cannot be resolved.
pub fn username() -> Option<String> {
    let from_uid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| real_uid(&status))
        .and_then(|uid| {
            std::fs::read_to_string("/etc/passwd")
                .ok()
                .and_then(|passwd| name_for_uid(&passwd, uid))
        });
    from_uid.or_else(|| {
        ["USER", "LOGNAME"]
            .iter()
            .filter_map(std::env::var_os)
            .filter_map(|value| value.into_string().ok())
            .map(|value| value.trim().to_string())
            .find(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod username_tests {
    use super::{name_for_uid, real_uid};

    #[test]
    fn resolves_the_account_from_uid_not_from_the_environment() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nlinsaftw:x:1000:1000::/home/linsaftw:/bin/zsh\n";
        assert_eq!(name_for_uid(passwd, 1000).as_deref(), Some("linsaftw"));
        assert_eq!(name_for_uid(passwd, 0).as_deref(), Some("root"));
        assert_eq!(name_for_uid(passwd, 4242), None);
        assert_eq!(name_for_uid("broken line\n::x\n", 0), None);
    }

    #[test]
    fn parses_the_real_uid_line() {
        assert_eq!(real_uid("Name:\tx\nUid:\t1000\t1001\t1000\t1000\n"), Some(1000));
        assert_eq!(real_uid("Name:\tx\n"), None);
    }
}
