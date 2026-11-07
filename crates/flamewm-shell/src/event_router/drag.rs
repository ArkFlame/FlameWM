use super::*;

#[derive(Debug, Clone)]
pub(crate) struct StartAppDrag {
    pub(super) app_id: flamewm_api::DesktopAppId,
    origin_x: f32,
    origin_y: f32,
    pub(super) dragging: bool,
}

impl StartAppDrag {
    pub(super) fn press(app_id: flamewm_api::DesktopAppId, x: f32, y: f32) -> Self {
        Self {
            app_id,
            origin_x: x,
            origin_y: y,
            dragging: false,
        }
    }

    pub(super) fn motion(&mut self, x: f32, y: f32) -> bool {
        if !self.dragging && (x - self.origin_x).hypot(y - self.origin_y) > START_DRAG_THRESHOLD_PX
        {
            self.dragging = true;
        }
        self.dragging
    }
}

pub(super) fn create_app_shortcut(app_id: &flamewm_api::DesktopAppId) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    let user_dirs = fs::read_to_string(home.join(".config/user-dirs.dirs")).ok();
    let desktop =
        flamewm_desktop_core::model::DesktopDirResolver::resolve(&home, user_dirs.as_deref());
    let catalog = flamewm_applications::ApplicationCatalog::discover()
        .map_err(|error| format!("discover application catalog: {error}"))?;
    let entry = catalog
        .find(app_id)
        .map_err(|error| format!("find application {}: {error}", app_id.as_str()))?;
    let source = entry.path();
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|_| {
            source
                .extension()
                .is_some_and(|extension| extension == "desktop")
        })
        .ok_or_else(|| "application has no desktop-entry file".to_owned())?;
    fs::create_dir_all(&desktop)
        .map_err(|error| format!("create desktop directory {}: {error}", desktop.display()))?;
    let destination = flamewm_desktop_core::file_actions::shortcut_destination(&desktop, file_name);
    fs::copy(source, &destination).map_err(|error| {
        format!(
            "copy application shortcut to {}: {error}",
            destination.display()
        )
    })?;
    Ok(destination)
}

/// Runs [`create_app_shortcut`] on a short-lived worker. The worker logs its own failure exactly as
/// the inline path did; the returned handle carries the result for callers that join it, and
/// dropping it detaches the worker. Only a failure to start the thread is returned here.
pub(super) fn spawn_app_shortcut(
    app_id: flamewm_api::DesktopAppId,
) -> std::io::Result<std::thread::JoinHandle<Result<PathBuf, String>>> {
    std::thread::Builder::new()
        .name("flamewm-desktop-shortcut".to_owned())
        .spawn(move || {
            let result = create_app_shortcut(&app_id);
            if let Err(error) = &result {
                eprintln!(
                    "flamewm-shell: create desktop shortcut for {}: {error}",
                    app_id.as_str()
                );
            }
            result
        })
}
