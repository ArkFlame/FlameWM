//! Default Applications page: one row per MIME group, cycled through the installed handlers.
//!
//! A group's handlers are the visible desktop entries that declare its first MIME type. Choosing
//! a handler writes only the group MIME types that handler declares, through the shared
//! `mimeapps.list` facade, so unrelated defaults are never rewritten.
use flamewm_settings_core::xdg_applications::{ApplicationSettings, InstalledHandler};

/// One Default Applications row: its label and the MIME types it governs, primary first.
pub struct Group {
    pub label: &'static str,
    pub mimes: &'static [&'static str],
}

/// Rows on the page; `ui/settings.html` declares one slot per group. The file manager is not a
/// row: the Desktop page's "Default file manager" control is its single source of truth.
pub const GROUPS: [Group; 7] = [
    Group {
        label: "Browser",
        mimes: &[
            "x-scheme-handler/http",
            "x-scheme-handler/https",
            "text/html",
        ],
    },
    Group {
        label: "Mail",
        mimes: &["x-scheme-handler/mailto"],
    },
    Group {
        label: "Terminal",
        mimes: &[
            "x-scheme-handler/terminal",
            "application/x-terminal-emulator",
        ],
    },
    Group {
        label: "Image viewer",
        mimes: &["image/png", "image/jpeg", "image/webp", "image/gif"],
    },
    Group {
        label: "Video",
        mimes: &["video/mp4", "video/x-matroska", "video/webm"],
    },
    Group {
        label: "Audio",
        mimes: &["audio/mpeg", "audio/flac", "audio/ogg"],
    },
    Group {
        label: "Text editor",
        mimes: &["text/plain"],
    },
];

/// The handler's display name when it is installed, otherwise the raw desktop ID.
fn label_for(handlers: &[InstalledHandler], id: Option<&str>) -> String {
    match id {
        None => "Not set".to_owned(),
        Some(id) => handlers
            .iter()
            .find(|handler| handler.id == id)
            .map_or_else(|| id.to_owned(), |handler| handler.name.clone()),
    }
}

/// Current default label for `slot`, or the empty state when the slot is unused.
pub fn current_label(slot: usize) -> Result<String, String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    current_label_with(&settings, slot)
}

fn current_label_with(settings: &ApplicationSettings, slot: usize) -> Result<String, String> {
    let group = GROUPS.get(slot).ok_or("unknown defaults row")?;
    let primary = group.mimes[0];
    let handlers = settings
        .installed_handlers(primary)
        .map_err(|e| e.to_string())?;
    let current = settings
        .default_application(primary)
        .map_err(|e| e.to_string())?;
    Ok(label_for(&handlers, current.as_deref()))
}

/// Advance `slot` to the next installed handler and persist it. Returns the new handler name.
pub fn cycle(slot: usize) -> Result<String, String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    cycle_with(&settings, slot)
}

fn cycle_with(settings: &ApplicationSettings, slot: usize) -> Result<String, String> {
    let group = GROUPS.get(slot).ok_or("unknown defaults row")?;
    let primary = group.mimes[0];
    let handlers = settings
        .installed_handlers(primary)
        .map_err(|e| e.to_string())?;
    if handlers.is_empty() {
        return Err(format!("No installed application opens {}", group.label));
    }
    let current = settings
        .default_application(primary)
        .map_err(|e| e.to_string())?;
    let next_index = handlers
        .iter()
        .position(|handler| Some(handler.id.as_str()) == current.as_deref())
        .map_or(0, |index| (index + 1) % handlers.len());
    let next = &handlers[next_index];
    for mime in group
        .mimes
        .iter()
        .filter(|mime| next.mimes.iter().any(|declared| declared == *mime))
    {
        settings
            .set_default_application(mime, &next.id)
            .map_err(|e| e.to_string())?;
    }
    Ok(next.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(tag: &str) -> (PathBuf, ApplicationSettings) {
        let root = std::env::temp_dir().join(format!(
            "flamewm-defaults-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("config");
        let data = root.join("data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(data.join("applications")).unwrap();
        let settings = ApplicationSettings {
            config_dirs: vec![config],
            data_dirs: vec![data],
            desktops: vec![],
        };
        (root, settings)
    }

    fn desktop(settings: &ApplicationSettings, id: &str, body: &str) {
        std::fs::write(settings.data_dirs[0].join("applications").join(id), body).unwrap();
    }

    #[test]
    fn cycle_walks_installed_handlers_and_writes_only_declared_mimes() {
        let (root, settings) = fixture("cycle");
        std::fs::write(
            settings.config_dirs[0].join("mimeapps.list"),
            "[Default Applications]\ntext/plain=editor.desktop;\nimage/png=old.desktop;\n",
        )
        .unwrap();
        desktop(
            &settings,
            "viewer-a.desktop",
            "[Desktop Entry]\nType=Application\nName=Viewer A\nMimeType=image/png;image/jpeg;\n",
        );
        desktop(
            &settings,
            "viewer-b.desktop",
            "[Desktop Entry]\nType=Application\nName=Viewer B\nMimeType=image/png;\n",
        );
        desktop(
            &settings,
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden\nMimeType=image/png;\nNoDisplay=true\n",
        );
        let image_slot = GROUPS
            .iter()
            .position(|g| g.label == "Image viewer")
            .unwrap();

        // No current default: the first handler is chosen; hidden entries never appear.
        assert_eq!(cycle_with(&settings, image_slot).unwrap(), "Viewer A");
        assert_eq!(
            current_label_with(&settings, image_slot).unwrap(),
            "Viewer A"
        );
        // Viewer A declares png and jpeg; the group's webp and gif lines stay untouched.
        let list = std::fs::read_to_string(settings.config_dirs[0].join("mimeapps.list")).unwrap();
        assert!(list.contains("image/jpeg=viewer-a.desktop;"));
        assert!(list.contains("text/plain=editor.desktop;"));
        assert!(!list.contains("image/webp="));

        assert_eq!(cycle_with(&settings, image_slot).unwrap(), "Viewer B");
        assert_eq!(cycle_with(&settings, image_slot).unwrap(), "Viewer A");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_handler_list_is_a_visible_error_and_writes_nothing() {
        let (root, settings) = fixture("empty");
        let mail = GROUPS.iter().position(|g| g.label == "Mail").unwrap();
        let error = cycle_with(&settings, mail).unwrap_err();
        assert_eq!(error, "No installed application opens Mail");
        assert!(!settings.config_dirs[0].join("mimeapps.list").exists());
        assert_eq!(current_label_with(&settings, mail).unwrap(), "Not set");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_slot_is_rejected() {
        let (root, settings) = fixture("slot");
        assert!(cycle_with(&settings, GROUPS.len()).is_err());
        assert!(current_label_with(&settings, GROUPS.len()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
