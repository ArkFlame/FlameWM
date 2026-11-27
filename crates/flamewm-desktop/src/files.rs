use super::*;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

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
    let data_home = flamewm_api::paths::xdg_dir("XDG_DATA_HOME", ".local/share")
        .unwrap_or_else(|| home.join(".local/share"));
    Trash::move_to_trash(path, &home, &data_home, uid)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

pub(crate) fn empty_home_trash() -> Result<(), String> {
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let data_home = flamewm_api::paths::xdg_dir("XDG_DATA_HOME", ".local/share")
        .unwrap_or_else(|| home.join(".local/share"));
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

/// One detached trash operation. Both variants are unbounded filesystem work on possibly remote
/// storage, so neither runs on the UI turn.
pub(crate) enum TrashWork {
    /// Move each entry to the freedesktop trash; every path reports its own outcome.
    MoveToTrash(Vec<PathBuf>),
    /// Empty the user's home trash.
    EmptyHome,
}

/// Off-turn trash executor. A desktop turn only submits work; the worker thread publishes its
/// outcome through a wake descriptor, and the reactor callback reports failures and arms one
/// rescan. Jobs are independent, so a blocked network mount cannot stall a later move.
pub(crate) struct TrashJobs {
    results: TrashOutcomes,
    wake_read: UnixStream,
    wake_write: Arc<UnixStream>,
}

/// Outcomes finished since the last drain, published from worker threads.
type TrashOutcomes = Arc<Mutex<Vec<Result<(), String>>>>;

impl TrashJobs {
    pub(crate) fn new() -> Result<Self, String> {
        let (wake_read, wake_write) =
            UnixStream::pair().map_err(|error| format!("create trash wake pair: {error}"))?;
        wake_read
            .set_nonblocking(true)
            .map_err(|error| format!("configure trash wake reader: {error}"))?;
        wake_write
            .set_nonblocking(true)
            .map_err(|error| format!("configure trash wake writer: {error}"))?;
        Ok(Self {
            results: Arc::new(Mutex::new(Vec::new())),
            wake_read,
            wake_write: Arc::new(wake_write),
        })
    }

    /// Read end that becomes readable as soon as one job finishes.
    #[must_use]
    pub(crate) fn wake_fd(&self) -> RawFd {
        self.wake_read.as_raw_fd()
    }

    /// Starts one job off the UI turn. Only a spawn failure is reported here.
    pub(crate) fn submit(&self, work: TrashWork) -> Result<(), String> {
        let results = Arc::clone(&self.results);
        let wake = Arc::clone(&self.wake_write);
        std::thread::Builder::new()
            .name("flamewm-desktop-trash".to_owned())
            .spawn(move || {
                let _ = flamewm_session_core::diagnostics::guard("desktop-trash", move || {
                    let finished = match work {
                        TrashWork::MoveToTrash(paths) => paths
                            .into_iter()
                            .map(|path| move_entry_to_trash(&path))
                            .collect(),
                        TrashWork::EmptyHome => vec![empty_home_trash()],
                    };
                    results
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(finished);
                    // A full wake pipe still leaves the earlier byte pending, and the drain that byte
                    // triggers reports every finished outcome, so a lost write cannot hide a result.
                    if let Ok(mut writer) = wake.try_clone() {
                        let _ = writer.write(&[1]);
                    }
                });
            })
            .map(|_| ())
            .map_err(|error| format!("start trash worker: {error}"))
    }

    /// Drains the wake byte and every outcome finished since the last call.
    pub(crate) fn drain(&mut self) -> Vec<Result<(), String>> {
        let mut buffer = [0_u8; 64];
        loop {
            match self.wake_read.read(&mut buffer) {
                Ok(read) if read > 0 => {}
                Ok(_) => break,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    eprintln!("desktop: trash wake drain failed: {error}");
                    break;
                }
            }
        }
        let mut finished = Vec::new();
        let mut held = self
            .results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        finished.append(&mut held);
        finished
    }
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
