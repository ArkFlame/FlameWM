//! Pane geometry: split rectangles, the pane tree of a tab, and split, close and focus over the active tab.

use super::{App, Pane, home_dir};
use crate::pty::Launch;
use std::path::PathBuf;

/// Cells between two panes of a split, drawn as a divider.
pub(super) const DIVIDER: usize = 1;

/// Smallest pane a split may produce. A split that cannot give both halves these cells is refused.
const MIN_PANE_COLS: usize = 2;

const MIN_PANE_ROWS: usize = 1;

/// Share of the split rectangle the first pane keeps when a split is created.
pub(super) const SPLIT_RATIO: f32 = 0.5;

/// Split direction. `Columns` puts the panes side by side (Split Vertical); `Rows` stacks them (Split Horizontal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Columns,
    Rows,
}

/// A pane's place in the grid, in cells. `y0` counts grid rows below the tab bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Rect {
    pub(super) fn contains(self, row: usize, col: usize) -> bool {
        row >= self.y0 && row < self.y0 + self.rows && col >= self.x0 && col < self.x0 + self.cols
    }
}

/// A tab's panes: leaves are shells, splits divide their rectangle between two subtrees.
pub enum PaneNode {
    Leaf(Box<Pane>),
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
    /// Set only while a leaf is moved out: the edit that empties a slot fills it before it returns.
    Vacant,
}

impl PaneNode {
    /// True when the tab shows a single shell, which is the only case that draws the startup banner.
    #[must_use]
    pub fn is_single(&self) -> bool {
        matches!(self, PaneNode::Leaf(_))
    }

    /// The leftmost leaf of the subtree.
    pub(super) fn leaf(&self) -> Option<&Pane> {
        match self {
            PaneNode::Leaf(pane) => Some(pane),
            PaneNode::Split { a, b, .. } => a.leaf().or_else(|| b.leaf()),
            PaneNode::Vacant => None,
        }
    }

    pub(super) fn first_id(&self) -> Option<u64> {
        self.leaf().map(|pane| pane.session.id)
    }

    pub(super) fn contains(&self, id: u64) -> bool {
        match self {
            PaneNode::Leaf(pane) => pane.session.id == id,
            PaneNode::Split { a, b, .. } => a.contains(id) || b.contains(id),
            PaneNode::Vacant => false,
        }
    }

    #[must_use]
    pub fn pane(&self, id: u64) -> Option<&Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.session.id == id).then_some(pane),
            PaneNode::Split { a, b, .. } => a.pane(id).or_else(|| b.pane(id)),
            PaneNode::Vacant => None,
        }
    }

    pub fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.session.id == id).then(|| pane.as_mut()),
            PaneNode::Split { a, b, .. } => match a.pane_mut(id) {
                Some(pane) => Some(pane),
                None => b.pane_mut(id),
            },
            PaneNode::Vacant => None,
        }
    }

    pub(super) fn leaves(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        match self {
            PaneNode::Leaf(pane) => out.push(pane.as_ref()),
            PaneNode::Split { a, b, .. } => {
                out.extend(a.leaves());
                out.extend(b.leaves());
            }
            PaneNode::Vacant => {}
        }
        out
    }

    pub(super) fn leaves_mut(&mut self) -> Vec<&mut Pane> {
        let mut out = Vec::new();
        match self {
            PaneNode::Leaf(pane) => out.push(pane.as_mut()),
            PaneNode::Split { a, b, .. } => {
                out.extend(a.leaves_mut());
                out.extend(b.leaves_mut());
            }
            PaneNode::Vacant => {}
        }
        out
    }

    /// Rectangles of every leaf, in tree order (left to right, top to bottom).
    #[must_use]
    pub fn rects(&self, rect: Rect) -> Vec<(u64, Rect)> {
        match self {
            PaneNode::Leaf(pane) => vec![(pane.session.id, rect)],
            PaneNode::Split { axis, ratio, a, b } => {
                let (first, second) = split_rect(rect, *axis, *ratio);
                let mut out = a.rects(first);
                out.extend(b.rects(second));
                out
            }
            PaneNode::Vacant => Vec::new(),
        }
    }

    /// Splits the leaf `id` along `axis`, putting `pane` second. The pane comes back when the leaf is not here.
    fn split_leaf(
        &mut self,
        id: u64,
        axis: Axis,
        ratio: f32,
        pane: Box<Pane>,
    ) -> Result<(), Box<Pane>> {
        match std::mem::replace(self, PaneNode::Vacant) {
            PaneNode::Leaf(existing) => {
                if existing.session.id == id {
                    *self = PaneNode::Split {
                        axis,
                        ratio,
                        a: Box::new(PaneNode::Leaf(existing)),
                        b: Box::new(PaneNode::Leaf(pane)),
                    };
                    Ok(())
                } else {
                    *self = PaneNode::Leaf(existing);
                    Err(pane)
                }
            }
            PaneNode::Split {
                axis: at,
                ratio: at_ratio,
                mut a,
                mut b,
            } => {
                let result = a
                    .split_leaf(id, axis, ratio, pane)
                    .or_else(|pane| b.split_leaf(id, axis, ratio, pane));
                *self = PaneNode::Split {
                    axis: at,
                    ratio: at_ratio,
                    a,
                    b,
                };
                result
            }
            PaneNode::Vacant => Err(pane),
        }
    }

    /// Removes the leaf `id`, collapsing its parent onto the surviving sibling. A node that held only that leaf is
    /// left `Vacant` for its own parent to collapse, so the caller must check the root afterwards.
    fn remove_leaf(&mut self, id: u64) -> Option<Removed> {
        match std::mem::replace(self, PaneNode::Vacant) {
            PaneNode::Leaf(pane) => {
                if pane.session.id == id {
                    return Some(Removed {
                        pane,
                        survivor: None,
                    });
                }
                *self = PaneNode::Leaf(pane);
                None
            }
            PaneNode::Split {
                axis,
                ratio,
                mut a,
                mut b,
            } => {
                let Some(removed) = a.remove_leaf(id).or_else(|| b.remove_leaf(id)) else {
                    *self = PaneNode::Split { axis, ratio, a, b };
                    return None;
                };
                *self = if matches!(*a, PaneNode::Vacant) {
                    *b
                } else if matches!(*b, PaneNode::Vacant) {
                    *a
                } else {
                    PaneNode::Split { axis, ratio, a, b }
                };
                Some(Removed {
                    survivor: removed.survivor.or_else(|| self.first_id()),
                    pane: removed.pane,
                })
            }
            PaneNode::Vacant => None,
        }
    }
}

/// A leaf taken out of a tab, with the leaf that should take its focus.
struct Removed {
    pane: Box<Pane>,
    survivor: Option<u64>,
}

/// Splits `rect` in two, leaving a one-cell divider between the halves. The halves may be smaller than a usable pane;
/// `split_fits` is the gate for that.
pub(super) fn split_rect(rect: Rect, axis: Axis, ratio: f32) -> (Rect, Rect) {
    match axis {
        Axis::Columns => {
            let usable = rect.cols.saturating_sub(DIVIDER);
            let first = first_part(usable, ratio, MIN_PANE_COLS);
            (
                Rect {
                    cols: first,
                    ..rect
                },
                Rect {
                    x0: rect.x0 + first + DIVIDER,
                    cols: usable.saturating_sub(first),
                    ..rect
                },
            )
        }
        Axis::Rows => {
            let usable = rect.rows.saturating_sub(DIVIDER);
            let first = first_part(usable, ratio, MIN_PANE_ROWS);
            (
                Rect {
                    rows: first,
                    ..rect
                },
                Rect {
                    y0: rect.y0 + first + DIVIDER,
                    rows: usable.saturating_sub(first),
                    ..rect
                },
            )
        }
    }
}

/// Cells for the first half: `ratio` of `usable`, at least `min` and leaving at least `min` for the second half.
fn first_part(usable: usize, ratio: f32, min: usize) -> usize {
    if usable < 2 * min {
        return usable / 2;
    }
    (((usable as f32) * ratio).round() as usize).clamp(min, usable - min)
}

/// Whether a split at `rect` leaves both halves a usable pane.
pub(super) fn split_fits(rect: Rect, axis: Axis) -> bool {
    let (first, second) = split_rect(rect, axis, SPLIT_RATIO);
    first.cols >= MIN_PANE_COLS
        && second.cols >= MIN_PANE_COLS
        && first.rows >= MIN_PANE_ROWS
        && second.rows >= MIN_PANE_ROWS
}

impl App {
    /// The whole grid below the tab bar, in cells. Panes are laid out inside it.
    pub(super) fn grid_rect(&self) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            cols: self.cols,
            rows: self.grid_rows,
        }
    }

    /// Gives every pane the size of its place in the tree, so each PTY follows its own rectangle.
    pub(super) fn relayout(&mut self) {
        let (cell_w, cell_h) = (self.cell_w, self.cell_h);
        let grid = self.grid_rect();
        for tab in &mut self.tabs {
            for (id, rect) in tab.root.rects(grid) {
                let Some(pane) = tab.root.pane_mut(id) else {
                    continue;
                };
                let size = (rect.cols.max(1), rect.rows.max(1));
                if size == pane.size {
                    continue;
                }
                pane.size = size;
                pane.session.resize(
                    size.0,
                    size.1,
                    (
                        (size.0 as f32 * cell_w) as u16,
                        (size.1 as f32 * cell_h) as u16,
                    ),
                );
            }
        }
    }

    /// Session id of the leaf that owns the keyboard, when the active tab still holds it.
    pub(super) fn focused_id(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        tab.root.contains(tab.focused).then_some(tab.focused)
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        let tab = self.tabs.get(self.active)?;
        tab.root.pane(tab.focused)
    }

    pub(super) fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        let id = self.focused_id()?;
        self.tabs.get_mut(self.active)?.root.pane_mut(id)
    }

    pub fn pane(&self, id: u64) -> Option<&Pane> {
        self.tabs.iter().find_map(|tab| tab.root.pane(id))
    }

    pub(super) fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        self.tabs.iter_mut().find_map(|tab| tab.root.pane_mut(id))
    }

    /// The pane holding a grid cell, with its rectangle.
    pub(super) fn pane_at(&self, grid_row: usize, col: usize) -> Option<(u64, Rect)> {
        let tab = self.tabs.get(self.active)?;
        tab.root
            .rects(self.grid_rect())
            .into_iter()
            .find(|(_, rect)| rect.contains(grid_row, col))
    }

    pub(super) fn pane_rect(&self, id: u64) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        tab.root
            .rects(self.grid_rect())
            .into_iter()
            .find(|(pane, _)| *pane == id)
            .map(|(_, rect)| rect)
    }

    pub(super) fn focus_pane(&mut self, id: u64) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.root.contains(id)
        {
            tab.focused = id;
        }
    }

    /// Splits the focused pane along `axis`, starting a shell in the focused shell's directory. Nothing happens when
    /// the halves would be too small to use.
    pub fn split_focused(&mut self, axis: Axis) -> Result<(), String> {
        let Some(focused) = self.focused_id() else {
            return Ok(());
        };
        let Some(rect) = self.pane_rect(focused) else {
            return Ok(());
        };
        if !split_fits(rect, axis) {
            return Ok(());
        }
        let (_, second) = split_rect(rect, axis, SPLIT_RATIO);
        let pane = self.start_pane(&Launch::shell(self.split_cwd()), second.cols, second.rows)?;
        let id = pane.session.id;
        let tab = &mut self.tabs[self.active];
        if tab
            .root
            .split_leaf(focused, axis, SPLIT_RATIO, pane)
            .is_err()
        {
            // The focused leaf vanished between the two steps: the new pane is dropped, and its PTY with it.
            return Ok(());
        }
        tab.focused = id;
        self.relayout();
        Ok(())
    }

    /// Directory a new pane starts in: the focused shell's when it is a readable directory, else home.
    fn split_cwd(&self) -> Option<PathBuf> {
        self.focused_pane()
            .and_then(|pane| pane.session.working_directory())
            .filter(|dir| dir.is_dir())
            .or_else(home_dir)
    }

    /// Closes the focused pane; the last pane of a tab closes the tab.
    pub fn close_pane(&mut self) {
        if let Some(id) = self.focused_id()
            && let Some(mut pane) = self.remove_pane(id)
        {
            pane.session.pty.kill();
        }
    }

    /// Takes the leaf `id` out of the tree, collapsing its parent onto the surviving sibling and giving that sibling
    /// the focus. The tab goes when its last leaf leaves. The removed pane is returned so the caller ends its process.
    pub(super) fn remove_pane(&mut self, id: u64) -> Option<Box<Pane>> {
        let index = self.tabs.iter().position(|tab| tab.root.contains(id))?;
        let tab = &mut self.tabs[index];
        let removed = tab.root.remove_leaf(id)?;
        if matches!(tab.root, PaneNode::Vacant) {
            self.tabs.remove(index);
            self.retarget_active(index);
            return Some(removed.pane);
        }
        if tab.focused == id {
            tab.focused = removed
                .survivor
                .or_else(|| tab.root.first_id())
                .unwrap_or(id);
        }
        self.relayout();
        Some(removed.pane)
    }
}
