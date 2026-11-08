use super::*;

/// A user-supplied Chromium secret-storage flag always wins over `FlameWM`'s policy.
pub(super) fn explicit_password_store(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--password-store=") {
            return Some(value.to_owned());
        }
        if arg == "--password-store" {
            return iter.next().cloned();
        }
    }
    None
}

/// Wallet name Chromium/KWallet opens. It is configuration metadata, never a secret.
pub(super) fn default_wallet_name() -> String {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let Some(path) = config.map(|dir| dir.join("kwalletrc")) else {
        return "kdewallet".to_owned();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return "kdewallet".to_owned();
    };
    let mut in_wallet = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_wallet = line.eq_ignore_ascii_case("[Wallet]");
            continue;
        }
        if in_wallet && let Some(name) = line.strip_prefix("Default Wallet=") {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_owned();
            }
        }
    }
    "kdewallet".to_owned()
}

pub(super) const WALLET_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const WALLET_WAIT_ATTEMPTS: usize = 3;
pub(super) const WALLET_WAIT_STEP: Duration = Duration::from_millis(250);

/// Chromium's `KWallet` backend opens `kdewallet`; `KWallet`'s configured default is
/// checked too so a renamed default cannot hide a locked browser wallet.
pub(super) fn wallet_names() -> Vec<String> {
    let mut names = vec!["kdewallet".to_owned()];
    let configured = default_wallet_name();
    if configured != "kdewallet" {
        names.push(configured);
    }
    names
}

/// Ask the existing `KWallet` daemon (never a new daemon or wallet) whether the
/// wallet is open. No secret material is read. `isOpen` is a standard,
/// non-destructive `KWallet` D-Bus method.
pub(super) fn wallet_ready(version: &str) -> Result<(), String> {
    let connection = dbus::blocking::Connection::new_session()
        .map_err(|error| format!("session bus unavailable: {error}"))?;
    let (service, path) = if version == "6" {
        ("org.kde.kwalletd6", "/modules/kwalletd6")
    } else {
        ("org.kde.kwalletd5", "/modules/kwalletd5")
    };
    let proxy = connection.with_proxy(service, path, WALLET_PROBE_TIMEOUT);
    for wallet in wallet_names() {
        let (open,): (bool,) = proxy
            .method_call("org.kde.KWallet", "isOpen", (wallet.as_str(),))
            .map_err(|error| format!("{service} is not reachable: {error}"))?;
        if !open {
            return Err(format!(
                "KWallet {version} is locked (wallet {wallet}); unlock it and retry"
            ));
        }
    }
    Ok(())
}

/// PAM unlocks asynchronously at login. Wait briefly for the same wallet to
/// become open, then fail closed instead of letting Chromium fall back to a
/// different password store.
pub(super) fn wait_for_wallet(version: &str) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..WALLET_WAIT_ATTEMPTS {
        match wallet_ready(version) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
        if attempt + 1 < WALLET_WAIT_ATTEMPTS {
            std::thread::sleep(WALLET_WAIT_STEP);
        }
    }
    Err(last)
}

pub(super) fn auto_wallet_version() -> FlameResult<Option<&'static str>> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let (installed5, installed6) = installed_kwallet_generations();
    let version = resolve_kwallet_version(&env, installed5, installed6);
    if version.is_none()
        && env
            .get("FLAMEWM_BRAVE_PASSWORD_STORE")
            .is_some_and(|value| value.starts_with("kwallet"))
    {
        return Err(FlameError::new(
            ErrorCode::Unavailable,
            "KWallet was requested but neither KWallet 5 nor 6 is installed",
        ));
    }
    Ok(version)
}

/// The single secret-storage policy for FlameWM-owned browser launches.
/// Brave gets the existing `KWallet` generation; Helium is left untouched unless
/// its original backend is explicitly opted in. Explicit user flags always win.
pub(super) fn browser_secret_version(
    program: &str,
    args: &[String],
) -> FlameResult<Option<&'static str>> {
    let brave = is_brave_program(program);
    let helium = is_helium_program(program);
    if !brave && !helium {
        return Ok(None);
    }
    if let Some(explicit) = explicit_password_store(args) {
        return Ok(match explicit.as_str() {
            "kwallet6" => Some("6"),
            "kwallet5" => Some("5"),
            "kwallet" => auto_wallet_version()?,
            // A user who chose basic, libsecret or none keeps that choice.
            "basic" | "gnome-libsecret" | "none" => None,
            _ => None,
        });
    }
    if helium {
        // Helium's portal/OSCrypt path is not assumed to be KWallet. Only an
        // explicit opt-in applies the KWallet contract to it.
        return Ok(
            match std::env::var("FLAMEWM_HELIUM_PASSWORD_STORE").as_deref() {
                Ok("kwallet6") => Some("6"),
                Ok("kwallet5") => Some("5"),
                _ => None,
            },
        );
    }
    auto_wallet_version()
}

pub(super) fn configure_browser_secret(
    command: &mut Command,
    program: &str,
    args: &[String],
) -> FlameResult<()> {
    let Some(version) = browser_secret_version(program, args)? else {
        return Ok(());
    };
    wait_for_wallet(version).map_err(|error| {
        FlameError::new(
            ErrorCode::Unavailable,
            format!(
                "browser launch paused to protect the existing profile: {error}. \
                 Unlock the existing KWallet wallet, then launch it again."
            ),
        )
    })?;
    // Child-process identity only. The session itself stays XDG_CURRENT_DESKTOP=FlameWM,
    // so KDE-only display services never compete with FlameWM.
    command.env("XDG_CURRENT_DESKTOP", "KDE");
    command.env("XDG_SESSION_DESKTOP", "KDE");
    command.env("KDE_SESSION_VERSION", version);
    Ok(())
}
