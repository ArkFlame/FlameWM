//! Freedesktop default file manager; no new `FlameWM` configuration key or parallel MIME database.
use std::path::PathBuf;
use std::process::Command;

const EXPLORER: &str = "flamewm-explorer.desktop";
const DOLPHIN: [&str; 2] = ["org.kde.dolphin.desktop", "dolphin.desktop"];

fn available(id: &str) -> bool {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let user =
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from);
    std::iter::once(user)
        .chain(std::env::split_paths(
            &std::env::var_os("XDG_DATA_DIRS")
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
        ))
        .any(|directory| directory.join("applications").join(id).is_file())
}

pub fn current() -> String {
    let Ok(output) = Command::new("xdg-mime")
        .args(["query", "default", "inode/directory"])
        .output()
    else {
        return "FlameWM Explorer".into();
    };
    let selected = String::from_utf8_lossy(&output.stdout);
    if selected.to_ascii_lowercase().contains("dolphin") {
        "Dolphin".into()
    } else if selected.contains(EXPLORER) {
        "FlameWM Explorer".into()
    } else if output.status.success() && !selected.trim().is_empty() {
        selected.trim().into()
    } else {
        "FlameWM Explorer".into()
    }
}

pub fn toggle() -> Result<String, String> {
    let target = if current() == "FlameWM Explorer" {
        DOLPHIN
            .into_iter()
            .find(|id| available(id))
            .ok_or("Dolphin is not installed")?
    } else {
        EXPLORER
    };
    if !available(target) {
        return Err(format!(
            "{target} is not installed. Build and install the full FlameWM package first."
        ));
    }
    let status = Command::new("xdg-mime")
        .args(["default", target, "inode/directory"])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("xdg-mime rejected the file-manager default".into());
    }
    Ok(current())
}
