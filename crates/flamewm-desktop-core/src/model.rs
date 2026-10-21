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
        self.rescan_with_grids(
            desktop_dir,
            default_output,
            &BTreeMap::from([(default_output.clone(), grid)]),
            grid,
        )
    }

    pub fn rescan_with_grids(
        &mut self,
        desktop_dir: &Path,
        default_output: &OutputId,
        grids: &BTreeMap<OutputId, GridConfig>,
        fallback_grid: GridConfig,
    ) -> FlameResult<bool> {
        let mut entries = fs::read_dir(desktop_dir)
            .map_err(|error| io_error("read desktop directory", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error("read desktop entry", error))?;
        // The converter publishes atomically; suppress only its private
        // in-progress files, not user-created dotfiles in general.
        entries.retain(|entry| !crate::conversion::is_temporary_convert_name(&entry.file_name()));
        entries.sort_by_key(|entry| entry.file_name());

        // Stored cells are reserved before any unstored entry is placed, so a
        // new entry never takes the cell of a stored entry that sorts after it.
        let mut reserved = BTreeMap::<OutputId, BTreeSet<Cell>>::new();
        for entry in &entries {
            if let Some(position) = self.positions.get(&entry.path()) {
                let output = migrated_output(&position.output, default_output);
                if let Some(grid) = grids.get(&output) {
                    reserved
                        .entry(output)
                        .or_default()
                        .insert(grid.clamp_cell(position.cell));
                }
            }
        }
        let mut occupied = reserved.clone();
        let mut assigned = BTreeMap::<OutputId, BTreeSet<Cell>>::new();
        let mut rebuilt = BTreeMap::new();
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = path.to_string_lossy().into_owned();
            let kind = classify(&entry)?;
            let position = match self.positions.get(&path).cloned() {
                Some(mut position) => {
                    position.output = migrated_output(&position.output, default_output);
                    if let Some(grid) = grids.get(&position.output) {
                        let wanted = grid.clamp_cell(position.cell);
                        let output_assigned = assigned.entry(position.output.clone()).or_default();
                        position.cell = if output_assigned.insert(wanted) {
                            wanted
                        } else {
                            let used = occupied.entry(position.output.clone()).or_default();
                            let cell = first_free(used, *grid);
                            used.insert(cell);
                            output_assigned.insert(cell);
                            cell
                        };
                    }
                    position
                }
                None => {
                    let grid = grids.get(default_output).copied().unwrap_or(fallback_grid);
                    let cells = occupied.entry(default_output.clone()).or_default();
                    let cell = first_free(cells, grid);
                    cells.insert(cell);
                    assigned
                        .entry(default_output.clone())
                        .or_default()
                        .insert(cell);
                    StoredPosition {
                        output: default_output.clone(),
                        cell,
                    }
                }
            };
            let cell = grids
                .get(&position.output)
                .map_or(position.cell, |grid| grid.clamp_cell(position.cell));
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
        for item in self
            .items
            .values_mut()
            .filter(|item| item.output == *output)
        {
            if let Some(position) = self.positions.get(&item.path) {
                item.cell = position.cell;
            }
        }
    }
}

fn migrated_output(output: &OutputId, default_output: &OutputId) -> OutputId {
    if output.as_str() == "default" {
        default_output.clone()
    } else {
        output.clone()
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
    let path = entry.path();
    if path.extension().and_then(|extension| extension.to_str()) == Some("desktop") {
        return Ok(DesktopItemKind::DesktopLauncher);
    }
    Ok(DesktopItemKind::File)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::unique_test_root;

    #[test]
    fn rescan_never_places_unstored_entry_on_a_stored_cell() {
        let root = unique_test_root("flamewm-model-free-cell");
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
    fn per_output_rescan_preserves_offline_identity_and_clamps_on_reconnect() {
        let root = unique_test_root("flamewm-model-output-layout");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("left-file"), b"").unwrap();
        fs::write(root.join("right-file"), b"").unwrap();
        let left = OutputId::new("DP-1");
        let right = OutputId::new("HDMI-1");
        let left_grid = GridConfig::compute(flamewm_api::Rect::new(0, 0, 960, 640), 100);
        let right_grid = GridConfig::compute(flamewm_api::Rect::new(960, 0, 2400, 1440), 100);
        let mut model = DesktopModel::default();
        model.set_position(
            root.join("left-file"),
            StoredPosition {
                output: left.clone(),
                cell: Cell::new(3, 2),
            },
        );
        model.set_position(
            root.join("right-file"),
            StoredPosition {
                output: right.clone(),
                cell: Cell::new(23, 14),
            },
        );
        model
            .rescan_with_grids(
                &root,
                &left,
                &BTreeMap::from([(left.clone(), left_grid), (right.clone(), right_grid)]),
                left_grid,
            )
            .unwrap();

        let right_position = model.positions()[&root.join("right-file")].clone();
        assert_eq!(right_position.output, right);
        assert_eq!(right_position.cell, Cell::new(23, 14));

        model
            .rescan_with_grids(
                &root,
                &left,
                &BTreeMap::from([(left.clone(), left_grid)]),
                left_grid,
            )
            .unwrap();
        assert_eq!(model.positions()[&root.join("right-file")], right_position);
        assert_eq!(
            model
                .items()
                .find(|item| item.display_name == "right-file")
                .unwrap()
                .output,
            right
        );

        model
            .rescan_with_grids(
                &root,
                &left,
                &BTreeMap::from([(left.clone(), left_grid), (right.clone(), right_grid)]),
                left_grid,
            )
            .unwrap();
        assert_eq!(model.positions()[&root.join("right-file")], right_position);

        let smaller_right = GridConfig::compute(flamewm_api::Rect::new(960, 0, 1920, 1080), 100);
        model
            .rescan_with_grids(
                &root,
                &left,
                &BTreeMap::from([(left.clone(), left_grid), (right.clone(), smaller_right)]),
                left_grid,
            )
            .unwrap();
        assert_eq!(
            model.positions()[&root.join("right-file")],
            StoredPosition {
                output: right,
                cell: Cell::new(19, 10),
            }
        );
        let _ = fs::remove_dir_all(&root);
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
