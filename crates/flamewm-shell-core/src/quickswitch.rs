//! FlameWM QuickSwitch policy for deterministic window cycling.
//!
//! Keyboard grabbing and the popup window remain desktop/native-renderer responsibilities. This
//! model only owns deterministic candidate filtering, advance, accept and cancel semantics.

use flamewm_api::WindowRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickSwitchOptions {
    pub all_workspaces: bool,
    pub include_minimized: bool,
}

impl Default for QuickSwitchOptions {
    fn default() -> Self {
        Self {
            all_workspaces: false,
            include_minimized: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickSwitchCandidate {
    pub window: WindowRef,
    pub title: String,
    pub app_class: String,
    pub workspace: usize,
    pub sticky: bool,
    pub minimized: bool,
    pub mapped: bool,
    pub skip_taskbar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickSwitchModel {
    candidates: Vec<QuickSwitchCandidate>,
    selected: Option<usize>,
    original_active: Option<WindowRef>,
}

impl Default for QuickSwitchModel {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            selected: None,
            original_active: None,
        }
    }
}

impl QuickSwitchModel {
    pub fn begin(
        &mut self,
        source: &[QuickSwitchCandidate],
        active_workspace: usize,
        active_window: Option<WindowRef>,
        options: QuickSwitchOptions,
        reverse: bool,
    ) -> Option<WindowRef> {
        self.candidates = source
            .iter()
            .filter(|candidate| !candidate.skip_taskbar)
            .filter(|candidate| {
                options.all_workspaces
                    || candidate.sticky
                    || candidate.workspace == active_workspace
            })
            .filter(|candidate| options.include_minimized || !candidate.minimized)
            .filter(|candidate| candidate.mapped || candidate.minimized)
            .cloned()
            .collect();
        self.original_active = active_window;
        if self.candidates.is_empty() {
            self.selected = None;
            return None;
        }
        if self.candidates.len() == 1 {
            self.selected = Some(0);
            return Some(self.candidates[0].window);
        }
        let current = active_window.and_then(|window| {
            self.candidates
                .iter()
                .position(|candidate| candidate.window == window)
        });
        let selected = match current {
            Some(index) if reverse => (index + self.candidates.len() - 1) % self.candidates.len(),
            Some(index) => (index + 1) % self.candidates.len(),
            None if reverse => self.candidates.len() - 1,
            None => 0,
        };
        self.selected = Some(selected);
        self.selected_window()
    }

    pub fn advance(&mut self, reverse: bool) -> Option<WindowRef> {
        let len = self.candidates.len();
        if len == 0 {
            self.selected = None;
            return None;
        }
        let current = self.selected.unwrap_or(0).min(len - 1);
        self.selected = Some(if reverse {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        });
        self.selected_window()
    }

    pub fn remove(&mut self, window: WindowRef) {
        let Some(selected) = self.selected else {
            return;
        };
        self.candidates
            .retain(|candidate| candidate.window != window);
        if self.candidates.is_empty() {
            self.selected = None;
            return;
        }
        self.selected = Some(selected.min(self.candidates.len() - 1));
    }

    #[must_use]
    pub fn selected_window(&self) -> Option<WindowRef> {
        self.selected
            .and_then(|index| self.candidates.get(index))
            .map(|candidate| candidate.window)
    }

    #[must_use]
    pub fn candidates(&self) -> &[QuickSwitchCandidate] {
        &self.candidates
    }

    pub fn accept(&mut self) -> Option<WindowRef> {
        let selected = self.selected_window();
        self.clear();
        selected
    }

    pub fn cancel(&mut self) -> Option<WindowRef> {
        let original = self.original_active;
        self.clear();
        original
    }

    fn clear(&mut self) {
        self.candidates.clear();
        self.selected = None;
        self.original_active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u64, workspace: usize) -> QuickSwitchCandidate {
        QuickSwitchCandidate {
            window: WindowRef::new(id, 1),
            title: format!("Window {id}"),
            app_class: "xterm".to_owned(),
            workspace,
            sticky: false,
            minimized: false,
            mapped: true,
            skip_taskbar: false,
        }
    }

    #[test]
    fn begins_after_current_active_window() {
        let source = vec![candidate(1, 0), candidate(2, 0), candidate(3, 0)];
        let mut model = QuickSwitchModel::default();
        assert_eq!(
            model.begin(
                &source,
                0,
                Some(WindowRef::new(2, 1)),
                QuickSwitchOptions::default(),
                false
            ),
            Some(WindowRef::new(3, 1))
        );
    }

    #[test]
    fn reverse_wraps_before_first_candidate() {
        let source = vec![candidate(1, 0), candidate(2, 0), candidate(3, 0)];
        let mut model = QuickSwitchModel::default();
        assert_eq!(
            model.begin(
                &source,
                0,
                Some(WindowRef::new(1, 1)),
                QuickSwitchOptions::default(),
                true
            ),
            Some(WindowRef::new(3, 1))
        );
    }

    #[test]
    fn current_workspace_filter_keeps_sticky_windows() {
        let mut sticky = candidate(2, 5);
        sticky.sticky = true;
        let source = vec![candidate(1, 0), sticky, candidate(3, 5)];
        let mut model = QuickSwitchModel::default();
        let _ = model.begin(&source, 0, None, QuickSwitchOptions::default(), false);
        assert_eq!(model.candidates().len(), 2);
    }

    #[test]
    fn minimized_candidate_can_be_excluded() {
        let mut minimized = candidate(2, 0);
        minimized.minimized = true;
        minimized.mapped = false;
        let source = vec![candidate(1, 0), minimized];
        let mut model = QuickSwitchModel::default();
        let _ = model.begin(
            &source,
            0,
            None,
            QuickSwitchOptions {
                all_workspaces: false,
                include_minimized: false,
            },
            false,
        );
        assert_eq!(model.candidates().len(), 1);
    }

    #[test]
    fn removal_reconciles_selected_index_without_stale_window() {
        let source = vec![candidate(1, 0), candidate(2, 0), candidate(3, 0)];
        let mut model = QuickSwitchModel::default();
        let _ = model.begin(
            &source,
            0,
            Some(WindowRef::new(1, 1)),
            QuickSwitchOptions::default(),
            false,
        );
        model.remove(WindowRef::new(2, 1));
        assert_ne!(model.selected_window(), Some(WindowRef::new(2, 1)));
    }

    #[test]
    fn cancel_returns_original_active_window() {
        let source = vec![candidate(1, 0), candidate(2, 0)];
        let mut model = QuickSwitchModel::default();
        let active = WindowRef::new(1, 1);
        let _ = model.begin(
            &source,
            0,
            Some(active),
            QuickSwitchOptions::default(),
            false,
        );
        assert_eq!(model.cancel(), Some(active));
    }
}
