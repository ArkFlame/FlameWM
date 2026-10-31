use super::*;

pub(crate) fn control_client() -> Result<ControlClient, String> {
    ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))
}

pub(crate) fn insert_workspace_after(
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
    state: &DesktopState,
) -> Result<(), String> {
    let dispatcher = mutation_dispatcher
        .ok_or_else(|| "workspace mutation dispatcher unavailable".to_owned())?;
    let snapshot = state.workspace.snapshot();
    dispatcher
        .submit(ControlRequest::InsertWorkspaceAfter {
            index: snapshot.count.saturating_sub(1),
            expected_revision: snapshot.revision,
        })
        .map(|_| ())
        .map_err(|error| error.message)
}

/// Collision-safe shortcut: `.desktop` sources are copied with a ` link`
/// suffix, other sources become symlinks. Pure path math plus direct fs
/// calls; no shell strings.
pub(crate) fn create_shortcut(state: &mut DesktopState) -> Result<(), String> {
    let selected = selected_menu_id(state);
    let Some(id) = selected else {
        return Ok(());
    };
    let source = state
        .model
        .items()
        .find(|item| item.id == id)
        .map(|item| item.path.clone());
    let Some(source) = source else {
        return Ok(());
    };
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "shortcut source has no file name".to_owned())?;
    let destination = shortcut_destination(&state.directory, &file_name);
    if source.extension().and_then(|value| value.to_str()) == Some("desktop") {
        fs::copy(&source, &destination)
            .map(|_| ())
            .map_err(|error| format!("copy {}: {error}", destination.display()))?;
    } else {
        std::os::unix::fs::symlink(&source, &destination)
            .map_err(|error| format!("symlink {}: {error}", destination.display()))?;
    }
    rescan_desktop(state)?;
    refresh_launchers(state);
    state.selection.clear();
    state
        .selection
        .select(destination.to_string_lossy().into_owned());
    Ok(())
}

/// Freedesktop Trash replaces irreversible deletion. The user's home is
/// authoritative for `XDG_DATA_HOME`, and the UID comes from its metadata.
pub(crate) fn move_entry_to_trash(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let uid = fs::metadata(&home)
        .map_err(|error| format!("stat HOME: {error}"))?
        .uid();
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from);
    Trash::move_to_trash(path, &home, &data_home, uid)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

pub(crate) fn empty_home_trash() -> Result<(), String> {
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from);
    let location = TrashLocation {
        scope: TrashScope::Home,
        top_directory: None,
        base: data_home.join("Trash"),
    };
    Trash::empty(&location)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

pub(crate) fn blank_row_id(action: BlankDesktopAction) -> &'static str {
    match action {
        BlankDesktopAction::OpenTerminal => "menu-row-terminal",
        BlankDesktopAction::CreateNewFolder => "menu-row-folder",
        BlankDesktopAction::NewStickyNote => "menu-row-note",
        BlankDesktopAction::AddWidgets => "menu-row-widgets",
        BlankDesktopAction::AddVirtualDesktop => "menu-row-desktop",
        BlankDesktopAction::DesktopAndWallpaper => "menu-row-settings",
    }
}

pub(crate) fn entry_row_id(action: EntryMenuAction) -> &'static str {
    match action {
        EntryMenuAction::EmptyTrash => "menu-row-empty-trash",
        EntryMenuAction::Rename => "menu-row-rename",
        EntryMenuAction::CreateShortcut => "menu-row-shortcut",
        EntryMenuAction::Convert => "menu-row-convert",
        EntryMenuAction::Delete => "menu-row-delete",
    }
}
