use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{FlameResult, OutputId};

use crate::atomic_file::io_error;
use crate::layout::{Cell, GridConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopItemKind {
    Directory,
    DesktopLauncher,
    File,
    Symlink,
    TrashPseudo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopItem {
    pub id: String,
    pub path: PathBuf,
    pub display_name: String,
    pub kind: DesktopItemKind,
    pub output: OutputId,
    pub cell: Cell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPosition {
    pub output: OutputId,
    pub cell: Cell,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DesktopModel {
    items: BTreeMap<String, DesktopItem>,
    positions: BTreeMap<PathBuf, StoredPosition>,
}

impl DesktopModel {
    #[must_use]
    pub fn items(&self) -> impl Iterator<Item = &DesktopItem> {
        self.items.values()
    }

    #[must_use]
    pub fn positions(&self) -> &BTreeMap<PathBuf, StoredPosition> {
        &self.positions
    }

    pub fn set_position(&mut self, path: PathBuf, position: StoredPosition) {
        self.positions.insert(path, position);
    }

    pub fn migrate_rename(&mut self, old: &Path, new: &Path) {
        if let Some(position) = self.positions.remove(old) {
            self.positions.insert(new.to_path_buf(), position);
        }
    }

    pub fn rescan(
        &mut self,
        desktop_dir: &Path,
        default_output: &OutputId,
        grid: GridConfig,
    ) -> FlameResult<bool> {
        let mut entries = fs::read_dir(desktop_dir)
            .map_err(|error| io_error("read desktop directory", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error("read desktop entry", error))?;
        entries.sort_by_key(|entry| entry.file_name());

        // Stored cells are reserved before any unstored entry is placed, so a
        // new entry never takes the cell of a stored entry that sorts after it.
        let mut occupied: BTreeSet<Cell> = entries
            .iter()
            .filter_map(|entry| self.positions.get(&entry.path()))
            .map(|position| grid.clamp_cell(position.cell))
            .collect();
        let mut rebuilt = BTreeMap::new();
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = path.to_string_lossy().into_owned();
            let kind = classify(&entry)?;
            let position = match self.positions.get(&path).cloned() {
                Some(position) => position,
                None => {
                    let cell = first_free(&occupied, grid);
                    occupied.insert(cell);
                    StoredPosition {
                        output: default_output.clone(),
                        cell,
                    }
                }
            };
            let cell = grid.clamp_cell(position.cell);
            self.positions.insert(
                path.clone(),
                StoredPosition {
                    output: position.output.clone(),
                    cell,
                },
            );
            rebuilt.insert(
                id.clone(),
                DesktopItem {
                    id,
                    path,
                    display_name: name,
                    kind,
                    output: position.output,
                    cell,
                },
            );
        }

        let changed = rebuilt != self.items;
        self.items = rebuilt;
        Ok(changed)
    }

    pub fn reflow(&mut self, output: &OutputId, grid: GridConfig) {
        let mut occupied = BTreeSet::new();
        let paths = self
            .positions
            .iter()
            .filter_map(|(path, position)| (position.output == *output).then_some(path.clone()))
            .collect::<Vec<_>>();
        for path in paths {
            let next = first_free(&occupied, grid);
            occupied.insert(next);
            if let Some(position) = self.positions.get_mut(&path) {
                position.cell = next;
            }
        }
        for item in self.items.values_mut() {
            if item.output == *output {
                if let Some(position) = self.positions.get(&item.path) {
                    item.cell = position.cell;
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopDirResolver;

impl DesktopDirResolver {
    #[must_use]
    pub fn resolve(home: &Path, user_dirs: Option<&str>) -> PathBuf {
        let Some(text) = user_dirs else {
            return home.join("Desktop");
        };
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some(value) = line.strip_prefix("XDG_DESKTOP_DIR=") else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            if let Some(suffix) = value.strip_prefix("$HOME") {
                return home.join(suffix.trim_start_matches('/'));
            }
            if let Some(suffix) = value.strip_prefix("${HOME}") {
                return home.join(suffix.trim_start_matches('/'));
            }
            let path = PathBuf::from(value);
            if path.is_absolute() {
                return path;
            }
        }
        home.join("Desktop")
    }
}

#[must_use]
pub fn first_free(occupied: &BTreeSet<Cell>, grid: GridConfig) -> Cell {
    for column in 0..grid.columns {
        for row in 0..grid.rows {
            let cell = Cell::new(column, row);
            if !occupied.contains(&cell) {
                return cell;
            }
        }
    }
    Cell::new(0, 0)
}

fn classify(entry: &fs::DirEntry) -> FlameResult<DesktopItemKind> {
    let file_type = entry
        .file_type()
        .map_err(|error| io_error("query desktop entry type", error))?;
    if file_type.is_symlink() {
        return Ok(DesktopItemKind::Symlink);
    }
    if file_type.is_dir() {
        return Ok(DesktopItemKind::Directory);
    }
    if entry
        .path()
        .extension()
        .and_then(|extension| extension.to_str())
        == Some("desktop")
    {
        return Ok(DesktopItemKind::DesktopLauncher);
    }
    Ok(DesktopItemKind::File)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescan_never_places_unstored_entry_on_a_stored_cell() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-model-free-cell-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // "alpha" sorts before "zed": the old single pass gave alpha the free cell
        // (0,0) before zed's stored cell was reserved, so both shared it.
        fs::write(root.join("alpha"), b"").unwrap();
        fs::write(root.join("zed"), b"").unwrap();
        let grid = GridConfig::compute(flamewm_api::Rect::new(0, 0, 1280, 800), 100);
        let output = OutputId::new("default");
        let zed = Cell::new(0, 0);
        let mut model = DesktopModel::default();
        model.set_position(
            root.join("zed"),
            StoredPosition {
                output: output.clone(),
                cell: zed,
            },
        );
        model.rescan(&root, &output, grid).unwrap();
        let alpha_cell = model
            .items()
            .find(|item| item.display_name == "alpha")
            .map(|item| item.cell)
            .unwrap();
        let zed_cell = model
            .items()
            .find(|item| item.display_name == "zed")
            .map(|item| item.cell)
            .unwrap();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(zed_cell, zed);
        assert_ne!(alpha_cell, zed);
    }

    #[test]
    fn desktop_dir_expands_only_home_forms_without_shell_evaluation() {
        let home = Path::new("/home/juan");
        assert_eq!(
            DesktopDirResolver::resolve(home, Some("XDG_DESKTOP_DIR=\"$HOME/My Desktop\"")),
            PathBuf::from("/home/juan/My Desktop")
        );
        assert_eq!(
            DesktopDirResolver::resolve(home, Some("XDG_DESKTOP_DIR=\"$(touch /tmp/nope)\"")),
            PathBuf::from("/home/juan/Desktop")
        );
    }
}
