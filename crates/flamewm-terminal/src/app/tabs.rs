//! Tab lifecycle, the tab bar with its hit areas, and inline tab rename.

use super::layout::PaneNode;
use super::{App, Tab, View};
use crate::pty::Launch;
use crate::view::Run;
use crate::view::text_run;

/// Inline tab rename in progress: the tab index and the text typed so far.
pub struct Rename {
    pub tab: usize,
    pub text: String,
}

/// Tab bar targets, in window coordinates on row 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Close(usize),
    NewTab,
    Settings,
}

impl App {
    /// Starts a tab. `launch` defaults to the user's shell in `cwd`.
    pub fn open_tab(&mut self, launch: Launch) -> Result<(), String> {
        let pane = self.start_pane(&launch, self.cols, self.grid_rows)?;
        let id = pane.session.id;
        // The banner belongs to the first tab only.
        self.banner = self.tabs.is_empty();
        self.tabs.push(Tab {
            id,
            title: None,
            root: PaneNode::Leaf(pane),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Opens a tab in the focused pane's working directory (the shell when the directory is unknown).
    pub fn open_tab_here(&mut self) -> Result<(), String> {
        let cwd = self
            .focused_pane()
            .and_then(|pane| pane.session.working_directory());
        self.open_tab(Launch::shell(cwd))
    }

    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let mut tab = self.tabs.remove(index);
            for pane in tab.root.leaves_mut() {
                pane.session.pty.kill();
            }
            self.retarget_active(index);
        }
    }

    /// Keeps `active` on the same tab after the tab at `index` was removed.
    pub(super) fn retarget_active(&mut self, index: usize) {
        if index < self.active {
            self.active -= 1;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Title shown for tab `index`: the name set by Rename Tab, else the focused shell's title, else a default.
    fn tab_title(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        let shell = tab
            .root
            .pane(tab.focused)
            .or_else(|| tab.root.leaf())
            .and_then(|pane| pane.session.screen.title.clone());
        let title = tab
            .title
            .clone()
            .or(shell)
            .unwrap_or_else(|| format!("Terminal {}", index + 1));
        title.chars().take(24).collect()
    }

    /// Tab bar layout: labels, the new-tab button and their hit areas on row 0. The tab strip shows only with more
    /// than one tab (or while renaming). Settings opens with Ctrl+, or by typing `/theme`.
    pub fn tab_bar(&self) -> (Vec<Run>, Vec<(usize, usize, Hit)>) {
        let fg = self.palette.foreground;
        let muted = self.palette.ansi[8];
        let accent_bg = Some(self.palette.selection);
        let mut runs = Vec::new();
        let mut hits = Vec::new();
        let mut col = 0;
        if let View::Settings(_) = self.view {
            let label = " ← Back ";
            runs.push(text_run(0, label, fg, accent_bg, true));
            hits.push((0, label.chars().count(), Hit::Settings));
            return (runs, hits);
        }
        if self.tabs.len() > 1 || self.rename.is_some() {
            for index in 0..self.tabs.len() {
                let active = index == self.active;
                let title = match &self.rename {
                    Some(rename) if rename.tab == index => format!("{}\u{258f}", rename.text),
                    _ => self.tab_title(index),
                };
                let label = format!(" {title} ");
                let width = label.chars().count() + 2;
                let bg = if active { accent_bg } else { None };
                let colour = if active { fg } else { muted };
                runs.push(text_run(col, &label, colour, bg, active));
                hits.push((col, col + label.chars().count(), Hit::Tab(index)));
                runs.push(text_run(
                    col + label.chars().count(),
                    "× ",
                    colour,
                    bg,
                    false,
                ));
                hits.push((col + label.chars().count(), col + width, Hit::Close(index)));
                col += width;
            }
            runs.push(text_run(col, " + ", fg, None, true));
            hits.push((col, col + 3, Hit::NewTab));
        }
        (runs, hits)
    }

    /// Ends an inline rename. A blank name clears the custom title, so the shell's title shows again.
    pub(super) fn finish_rename(&mut self, commit: bool) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        if let (true, Some(tab)) = (commit, self.tabs.get_mut(rename.tab)) {
            let name = rename.text.trim();
            tab.title = (!name.is_empty()).then(|| name.to_owned());
        }
    }

    pub(super) fn tab_hit(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(index) => {
                self.active = if index < self.tabs.len() {
                    index
                } else {
                    self.active
                }
            }
            Hit::Close(index) => self.close_tab(index),
            Hit::NewTab => {
                let _ = self.open_tab_here();
            }
            Hit::Settings => self.close_settings(),
        }
    }
}
