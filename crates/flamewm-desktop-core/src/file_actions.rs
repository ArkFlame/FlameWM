use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{ErrorCode, FlameError, FlameResult};

/// Treat dangling symlinks as occupied directory entries too.
fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

#[must_use]
pub fn next_new_folder_path(desktop: &Path) -> PathBuf {
    let first = desktop.join("New Folder");
    if !path_entry_exists(&first) {
        return first;
    }
    for number in 2_u32..=u32::MAX {
        let candidate = desktop.join(format!("New Folder ({number})"));
        if !path_entry_exists(&candidate) {
            return candidate;
        }
    }
    desktop.join("New Folder (overflow)")
}

pub fn create_new_folder(desktop: &Path) -> FlameResult<PathBuf> {
    let path = next_new_folder_path(desktop);
    fs::create_dir(&path).map_err(|error| {
        FlameError::new(
            ErrorCode::IoFailure,
            format!("create {}: {error}", path.display()),
        )
    })?;
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandIntent {
    pub program: String,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
}

#[must_use]
pub fn open_path(path: &Path) -> CommandIntent {
    CommandIntent {
        program: "xdg-open".to_owned(),
        argv: vec![path.to_string_lossy().into_owned()],
        cwd: None,
    }
}

#[must_use]
pub fn open_terminal(program: &str, desktop: &Path) -> CommandIntent {
    CommandIntent {
        program: program.to_owned(),
        argv: Vec::new(),
        cwd: Some(desktop.to_path_buf()),
    }
}

#[must_use]
pub fn desktop_settings() -> CommandIntent {
    CommandIntent {
        program: "flamewm-settings".to_owned(),
        argv: vec!["--page".to_owned(), "desktop".to_owned()],
        cwd: None,
    }
}

pub const DEFAULT_TERMINAL: &str = "x-terminal-emulator";

/// Terminal program without shell evaluation: `$TERMINAL` or compiled default.
#[must_use]
pub fn terminal_program() -> String {
    std::env::var("TERMINAL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TERMINAL.to_owned())
}

/// Entry context rows. Trash exposes Empty Trash only; normal entries expose
/// Rename, Create Shortcut, plus Delete (which the caller must confirm first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryMenuAction {
    EmptyTrash,
    Rename,
    CreateShortcut,
    Delete,
}

impl EntryMenuAction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::EmptyTrash => "Empty Trash",
            Self::Rename => "Rename",
            Self::CreateShortcut => "Create Shortcut",
            Self::Delete => "Delete",
        }
    }
}

#[must_use]
pub fn entry_context_menu(is_trash: bool) -> Vec<EntryMenuAction> {
    if is_trash {
        vec![EntryMenuAction::EmptyTrash]
    } else {
        vec![
            EntryMenuAction::Rename,
            EntryMenuAction::CreateShortcut,
            EntryMenuAction::Delete,
        ]
    }
}

/// Validate a proposed entry file name. Returns the trimmed name on success.
pub fn validate_rename_name(name: &str) -> FlameResult<String> {
    use std::path::Component;
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." || trimmed.contains(['/', '\0']) {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "invalid rename name",
        ));
    }
    let mut components = Path::new(trimmed).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(trimmed.to_owned()),
        _ => Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "invalid rename name",
        )),
    }
}

/// Rename `old` to `new` without replacing an existing destination.
/// Both paths must live in the same directory; no shell involved.
pub fn rename_entry_no_replace(old: &Path, new: &Path) -> FlameResult<()> {
    let old_parent = old
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "rename needs a parent"))?;
    let new_parent = new
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "rename needs a parent"))?;
    if old_parent != new_parent {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "rename must stay in the same directory",
        ));
    }
    let old_name = old
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "invalid rename source"))?;
    let new_name = new
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "invalid rename target"))?;
    // Use the validated (trimmed) name for both the check and the rename.
    let validated = validate_rename_name(new_name)?;
    if old_name == validated {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "rename name unchanged",
        ));
    }
    let target = old_parent.join(&validated);
    if !old.exists() && !old.is_symlink() {
        return Err(FlameError::new(
            ErrorCode::NotFound,
            "rename source not found",
        ));
    }
    if target.exists() || target.is_symlink() {
        return Err(FlameError::new(
            ErrorCode::Conflict,
            "rename target already exists",
        ));
    }
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        old,
        rustix::fs::CWD,
        &target,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| match error {
        rustix::io::Errno::NOENT => FlameError::new(ErrorCode::NotFound, "rename source not found"),
        rustix::io::Errno::EXIST => {
            FlameError::new(ErrorCode::Conflict, "rename target already exists")
        }
        rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP => {
            FlameError::new(ErrorCode::Unsupported, "atomic rename not supported")
        }
        other => FlameError::new(ErrorCode::IoFailure, format!("rename entry: {other}")),
    })
}

/// How a desktop item opens. `.desktop` entries launch via `DesktopEntry`;
/// regular files and directories use `xdg-open`. Never inverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherOpen {
    DesktopEntry,
    RegularFile,
}

#[must_use]
pub const fn launcher_open_kind(is_desktop_entry: bool) -> LauncherOpen {
    if is_desktop_entry {
        LauncherOpen::DesktopEntry
    } else {
        LauncherOpen::RegularFile
    }
}

/// Collision-safe shortcut destination inside the desktop directory.
/// `.desktop` sources are copied with a ` link` suffix before the extension;
/// other sources get a symlinked sibling of the same scheme. Pure path
/// computation, no shell strings.
#[must_use]
pub fn shortcut_destination(desktop: &Path, file_name: &str) -> PathBuf {
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => {
            (stem.to_owned(), Some(extension.to_owned()))
        }
        _ => (file_name.to_owned(), None),
    };
    let join = |stem: &str| match &extension {
        Some(extension) => desktop.join(format!("{stem}.{extension}")),
        None => desktop.join(stem),
    };
    let first = join(&format!("{stem} link"));
    if !path_entry_exists(&first) {
        return first;
    }
    for number in 2_u32..=u32::MAX {
        let candidate = join(&format!("{stem} link ({number})"));
        if !path_entry_exists(&candidate) {
            return candidate;
        }
    }
    join(&format!("{stem} link (overflow)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::unique_test_root;

    #[test]
    fn entry_menu_maps_trash_to_empty_trash_only() {
        assert_eq!(entry_context_menu(true), vec![EntryMenuAction::EmptyTrash]);
        assert_eq!(
            entry_context_menu(false),
            vec![
                EntryMenuAction::Rename,
                EntryMenuAction::CreateShortcut,
                EntryMenuAction::Delete
            ]
        );
    }

    #[test]
    fn launcher_open_selects_desktop_entry_for_desktop_files() {
        assert_eq!(launcher_open_kind(true), LauncherOpen::DesktopEntry);
        assert_eq!(launcher_open_kind(false), LauncherOpen::RegularFile);
    }

    #[test]
    fn shortcut_destination_appends_link_suffix_collision_free() {
        let root = unique_test_root("flamewm-shortcut");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            shortcut_destination(&root, "app.desktop"),
            root.join("app link.desktop")
        );
        fs::write(root.join("app link.desktop"), b"x").unwrap();
        assert_eq!(
            shortcut_destination(&root, "app.desktop"),
            root.join("app link (2).desktop")
        );
        assert_eq!(
            shortcut_destination(&root, "notes"),
            root.join("notes link")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_renames_to_the_validated_trimmed_name() {
        let root = unique_test_root("flamewm-rename-trim");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"x").unwrap();

        let result = rename_entry_no_replace(&root.join("a.txt"), &root.join("b.txt "));

        let renamed_to_trimmed = root.join("b.txt").exists();
        let created_untrimmed = root.join("b.txt ").exists();
        let source_left = root.join("a.txt").exists();
        fs::remove_dir_all(&root).unwrap();

        assert!(result.is_ok(), "rename with padded name should succeed");
        assert!(renamed_to_trimmed, "entry must be renamed to trimmed name");
        assert!(!created_untrimmed, "untrimmed name must not be created");
        assert!(!source_left, "source must no longer exist under old name");
    }

    #[test]
    fn new_folder_and_shortcut_skip_dangling_symlink_collisions() {
        use std::os::unix::fs::symlink;

        let root = unique_test_root("flamewm-file-actions-collision");
        fs::create_dir_all(&root).unwrap();
        symlink(root.join("missing-folder-target"), root.join("New Folder")).unwrap();
        assert_eq!(
            create_new_folder(&root).unwrap(),
            root.join("New Folder (2)")
        );

        symlink(
            root.join("missing-link-target"),
            root.join("app link.desktop"),
        )
        .unwrap();
        assert_eq!(
            shortcut_destination(&root, "app.desktop"),
            root.join("app link (2).desktop")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_rejects_existing_dangling_symlink_without_clobbering() {
        use std::os::unix::fs::symlink;

        let root = unique_test_root("flamewm-rename-collision");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source file");
        let target = root.join("target file");
        fs::write(&source, b"source").unwrap();
        symlink(root.join("missing-target"), &target).unwrap();

        let result = rename_entry_no_replace(&source, &target);
        assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert!(
            fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
