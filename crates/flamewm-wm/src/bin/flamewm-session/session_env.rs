use super::*;

pub(super) fn add_app_compatibility_environment(
    processes: &mut [ProcessPlan],
    existing_env: &BTreeMap<String, String>,
) {
    let (installed5, installed6) = installed_kwallet_generations();
    if let Some(version) = resolve_kwallet_version(existing_env, installed5, installed6) {
        // The explicit Brave compatibility overlay and PAM handoff need the
        // selected generation; generic session children do not impersonate KDE.
        for process in processes {
            process
                .environment
                .insert("KDE_SESSION_VERSION".to_owned(), version.to_owned());
        }
    }
}

pub(super) fn add_keyring_environment(
    processes: &mut [ProcessPlan],
    keyring_environment: &BTreeMap<String, String>,
) {
    for process in processes {
        process.environment.extend(keyring_environment.clone());
    }
}

/// `KWallet` is unlocked by the PAM handoff above. Without it, start gnome-keyring the way
/// cosmic-session does so libsecret clients find a Secret Service. Never touches wallets.
pub(super) fn start_session_keyring(keyring: Keyring) -> BTreeMap<String, String> {
    start_session_keyring_with(keyring, || {
        Command::new("gnome-keyring-daemon")
            .args(["--start", "--components=pkcs11,secrets,ssh"])
            .output()
    })
}

pub(super) fn start_session_keyring_with(
    keyring: Keyring,
    start: impl FnOnce() -> std::io::Result<Output>,
) -> BTreeMap<String, String> {
    match keyring {
        Keyring::KWallet(version) => {
            eprintln!("{SESSION_LOG_PREFIX}: keyring: KWallet {version} (PAM handoff only)");
            BTreeMap::new()
        }
        Keyring::GnomeKeyring => match start() {
            Ok(output) if output.status.success() => {
                let environment = std::str::from_utf8(&output.stdout)
                    .ok()
                    .and_then(parse_gnome_keyring_environment);
                if let Some(environment) = environment {
                    eprintln!("{SESSION_LOG_PREFIX}: keyring: gnome-keyring started");
                    return environment;
                }
                eprintln!(
                    "{SESSION_LOG_PREFIX}: keyring: gnome-keyring returned invalid startup environment"
                );
                BTreeMap::new()
            }
            Ok(output) => {
                eprintln!(
                    "{SESSION_LOG_PREFIX}: keyring: gnome-keyring failed: {}",
                    output.status
                );
                BTreeMap::new()
            }
            Err(error) => {
                eprintln!("{SESSION_LOG_PREFIX}: keyring: gnome-keyring: {error}");
                BTreeMap::new()
            }
        },
        Keyring::Absent => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: keyring: none installed; browsers use their own store"
            );
            BTreeMap::new()
        }
    }
}

pub(super) const ALLOCATOR_TUNING: [(&str, &str); 3] = [
    ("MALLOC_ARENA_MAX", "2"),
    ("MALLOC_TRIM_THRESHOLD_", "131072"),
    ("MALLOC_MMAP_THRESHOLD_", "131072"),
];

/// Allocator-tuning pairs whose key is not already set; a user-provided value always wins.
pub(super) fn allocator_tuning_defaults(is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    ALLOCATOR_TUNING
        .into_iter()
        .filter(|(key, _)| !is_set(key))
        .collect()
}

/// Publish the SAME session identity and display coordinates to services
/// activated later by Dolphin, Brave, xdg-desktop-portal or `KWallet`.
/// No credential access and no global user-config mutation.
pub(super) fn activation_assignments(
    environment: &BTreeMap<String, String>,
    inherited: &BTreeMap<String, String>,
) -> Vec<String> {
    pub(super) const KEYS: &[&str] = &[
        "DISPLAY",
        "XAUTHORITY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_DATA_DIRS",
        "XDG_CONFIG_DIRS",
        "XDG_MENU_PREFIX",
        "QT_QPA_PLATFORMTHEME",
        "KDE_FULL_SESSION",
        "KDE_SESSION_VERSION",
        "KDE_SESSION_UID",
        "GNOME_KEYRING_CONTROL",
        "GPG_AGENT_INFO",
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
    ];
    KEYS.iter()
        .filter_map(|key| {
            // The selected wallet generation is for explicit Brave/PAM compatibility only.
            if matches!(
                *key,
                "KDE_FULL_SESSION" | "KDE_SESSION_VERSION" | "KDE_SESSION_UID"
            ) {
                return Some(format!("{key}="));
            }
            environment
                .get(*key)
                .or_else(|| inherited.get(*key))
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={value}"))
        })
        .collect()
}

pub(super) fn clear_kde_session_identity(environment: &mut BTreeMap<String, String>) {
    for key in KDE_SESSION_IDENTITY {
        environment.remove(key);
    }
}

pub(super) fn apply_session_environment(command: &mut Command, environment: &BTreeMap<String, String>) {
    command.envs(environment);
    for key in KDE_SESSION_IDENTITY {
        command.env_remove(key);
    }
}

#[derive(Debug)]
pub(super) struct ActivationEnvironmentSnapshot {
    pub(super) keys: Vec<String>,
    pub(super) previous: Vec<String>,
    pub(super) restored: bool,
}

impl ActivationEnvironmentSnapshot {
    pub(super) fn from_show_environment(keys: Vec<String>, output: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in output.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if keys.iter().any(|requested| requested.as_str() == key) {
                    values.insert(key.to_owned(), value.to_owned());
                }
            }
        }
        let previous = keys
            .iter()
            .filter_map(|key| values.get(key).map(|value| format!("{key}={value}")))
            .collect();
        Self {
            keys,
            previous,
            restored: false,
        }
    }

    pub(super) fn capture(keys: Vec<String>) -> Result<Self, String> {
        if !Command::new("busctl")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Err("busctl unavailable".to_owned());
        }
        let output = Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .map_err(|_| "systemctl user environment unavailable".to_owned())?;
        if !output.status.success() {
            return Err(format!(
                "systemctl show-environment exited with {}",
                output.status
            ));
        }
        Ok(Self::from_show_environment(
            keys,
            &String::from_utf8_lossy(&output.stdout),
        ))
    }

    pub(super) fn busctl_arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "--user".to_owned(),
            "call".to_owned(),
            "org.freedesktop.systemd1".to_owned(),
            "/org/freedesktop/systemd1".to_owned(),
            "org.freedesktop.systemd1.Manager".to_owned(),
            "UnsetAndSetEnvironment".to_owned(),
            "as".to_owned(),
            self.keys.len().to_string(),
        ];
        arguments.extend(self.keys.iter().cloned());
        arguments.push("as".to_owned());
        arguments.push(self.previous.len().to_string());
        arguments.extend(self.previous.iter().cloned());
        arguments
    }

    pub(super) fn dbus_restore_assignments(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|key| {
                let value = self
                    .previous
                    .iter()
                    .find_map(|assignment| {
                        let (name, value) = assignment.split_once('=')?;
                        (name == key.as_str()).then_some(value)
                    })
                    .unwrap_or("");
                format!("{key}={value}")
            })
            .collect()
    }

    pub(super) fn restore(&mut self) -> Result<(), String> {
        self.restore_with(run_bounded_activation_command)
    }

    pub(super) fn restore_with(
        &mut self,
        mut run: impl FnMut(&str, &[String]) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        // dbus-daemon has no unset API, so restore known values and clear
        // previously absent keys with empty values before atomically restoring
        // systemd's exact set/unset state.
        let dbus_assignments = self.dbus_restore_assignments();
        let dbus_result = run("dbus-update-activation-environment", &dbus_assignments);
        let systemd_arguments = self.busctl_arguments();
        let systemd_result = run("busctl", &systemd_arguments);
        match (dbus_result, systemd_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(dbus), Ok(())) => Err(format!("D-Bus restore failed: {dbus}")),
            (Ok(()), Err(systemd)) => Err(format!("systemd restore failed: {systemd}")),
            (Err(dbus), Err(systemd)) => Err(format!(
                "D-Bus restore failed: {dbus}; systemd restore failed: {systemd}"
            )),
        }
    }
}

pub(super) fn run_bounded_activation_command(program: &str, arguments: &[String]) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("{program} could not start"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("exited with {status}")),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("child status unavailable".to_owned());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out after 5s".to_owned());
            }
        }
    }
}

impl Drop for ActivationEnvironmentSnapshot {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("{SESSION_LOG_PREFIX}: activation environment restore failed: {error}");
        }
    }
}

pub(super) fn combine_activation_restore_result(
    session_result: Result<(), String>,
    restore_result: Result<(), String>,
) -> Result<(), String> {
    match (session_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(session), Ok(())) => Err(session),
        (Ok(()), Err(restore)) => Err(format!("activation environment restore failed: {restore}")),
        (Err(session), Err(restore)) => Err(format!(
            "{session}; activation environment restore failed: {restore}"
        )),
    }
}

pub(super) fn activation_assignment_keys(assignments: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    for assignment in assignments {
        if let Some((key, _)) = assignment.split_once('=') {
            if !keys
                .iter()
                .any(|existing: &String| existing.as_str() == key)
            {
                keys.push(key.to_owned());
            }
        }
    }
    keys
}

pub(super) fn sync_activation_environment(
    environment: &BTreeMap<String, String>,
) -> Option<ActivationEnvironmentSnapshot> {
    let inherited = env::vars().collect::<BTreeMap<_, _>>();
    let assignments = activation_assignments(environment, &inherited);
    let keys = activation_assignment_keys(&assignments);
    let snapshot = match ActivationEnvironmentSnapshot::capture(keys) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: activation environment not synchronized: unable to snapshot user manager ({error})"
            );
            return None;
        }
    };
    let mut command = Command::new("dbus-update-activation-environment");
    command.arg("--systemd").args(assignments);
    match command.status() {
        Ok(status) if status.success() => (),
        Ok(status) => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: D-Bus activation environment not synchronized: {status}"
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: dbus-update-activation-environment unavailable; portal session integration degraded"
            );
        }
        Err(error) => eprintln!("{SESSION_LOG_PREFIX}: activation environment: {error}"),
    }
    Some(snapshot)
}
