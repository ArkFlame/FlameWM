//! Opt-in per-output workspace model.
//!
//! The rest of the workspace machinery keeps a single `current` workspace for the whole
//! session: `WindowModel::switch_workspace` moves one global cursor and every window is
//! visible exactly when it sits on that cursor's workspace. This module is a *pure* alternative for
//! callers that opt in: with [`WorkspaceMode::PerOutput`] each output owns its own current workspace
//! and a window belongs to one output, so the visible set is computed per output. With the default
//! [`WorkspaceMode::Global`] every operation collapses back to the single global cursor, matching
//! today's behaviour exactly.
//!
//! Nothing here talks to a display, backend, or the shell. It is state plus transitions; the owner
//! of the real window model decides whether to wire it in.

use std::collections::HashMap;
use std::hash::Hash;

/// Default workspace count, matching `WindowModel::default()`'s `new(2)`.
const DEFAULT_WORKSPACE_COUNT: usize = 4;

/// Which workspace model is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum WorkspaceMode {
    /// One workspace cursor for the whole session — the behaviour used everywhere today.
    #[default]
    Global,
    /// One workspace cursor per output; windows are grouped by output as well.
    PerOutput,
}

/// Pure workspace state, generic over the output and window identifier types.
///
/// `OutputId` and `WindowId` only need equality, hashing, and cloning; the model never interprets
/// them. Windows retain stacking order (bottom to top, as in `WindowModel`) so visibility queries
/// can return the topmost-first order backends expect.
#[derive(Debug, Clone)]
pub struct PerOutputWorkspaces<OutputId, WindowId>
where
    OutputId: Eq + Hash + Clone,
    WindowId: Eq + Hash + Clone,
{
    mode: WorkspaceMode,
    count: usize,
    primary: OutputId,
    /// The single visible workspace used in [`WorkspaceMode::Global`].
    global_current: usize,
    /// Visible workspace per output, used only in [`WorkspaceMode::PerOutput`].
    per_output_current: HashMap<OutputId, usize>,
    /// Stacking order, bottom to top.
    windows: Vec<WindowId>,
    window_output: HashMap<WindowId, OutputId>,
    window_workspace: HashMap<WindowId, usize>,
}

impl<OutputId, WindowId> PerOutputWorkspaces<OutputId, WindowId>
where
    OutputId: Eq + Hash + Clone,
    WindowId: Eq + Hash + Clone,
{
    /// Build a model with an explicit mode and workspace count.
    ///
    /// `workspace_count` is clamped to `1..=MAX_WORKSPACES`, exactly as `WindowModel::new` clamps
    /// it. `primary` is the rehome target when an output is removed.
    pub fn new(mode: WorkspaceMode, workspace_count: usize, primary: OutputId) -> Self {
        Self {
            mode,
            count: workspace_count.clamp(1, flamewm_api::workspace::MAX_WORKSPACES),
            primary,
            global_current: 0,
            per_output_current: HashMap::new(),
            windows: Vec::new(),
            window_output: HashMap::new(),
            window_workspace: HashMap::new(),
        }
    }

    /// A model that behaves exactly like today's single-global-workspace model.
    pub fn global(primary: OutputId) -> Self {
        Self::new(WorkspaceMode::Global, DEFAULT_WORKSPACE_COUNT, primary)
    }

    /// A model that opts into one current workspace per output.
    pub fn per_output(primary: OutputId) -> Self {
        Self::new(WorkspaceMode::PerOutput, DEFAULT_WORKSPACE_COUNT, primary)
    }

    /// The active mode.
    pub const fn mode(&self) -> WorkspaceMode {
        self.mode
    }

    /// The validated workspace count.
    pub const fn workspace_count(&self) -> usize {
        self.count
    }

    /// The output windows are rehomed to when their output disappears.
    pub fn primary_output(&self) -> &OutputId {
        &self.primary
    }

    /// Workspace currently visible on `output`.
    ///
    /// In [`WorkspaceMode::Global`] `output` is ignored and the one global workspace is returned.
    /// In [`WorkspaceMode::PerOutput`] an output that was never seen reads as workspace `0`.
    pub fn current_workspace(&self, output: &OutputId) -> usize {
        match self.mode {
            WorkspaceMode::Global => self.global_current,
            WorkspaceMode::PerOutput => self.per_output_current.get(output).copied().unwrap_or(0),
        }
    }

    /// Switch `output` to `ws`. Returns `false` when `ws` is out of range or already current.
    ///
    /// In [`WorkspaceMode::Global`] `output` is ignored: this moves the single global workspace,
    /// which is what `WindowModel::switch_workspace` does.
    pub fn switch(&mut self, output: OutputId, ws: usize) -> bool {
        if ws >= self.count {
            return false;
        }
        match self.mode {
            WorkspaceMode::Global => {
                if ws == self.global_current {
                    return false;
                }
                self.global_current = ws;
                true
            }
            WorkspaceMode::PerOutput => {
                let current = self.per_output_current.entry(output).or_insert(0);
                if *current == ws {
                    return false;
                }
                *current = ws;
                true
            }
        }
    }

    /// Add `win` on `output`, placed on that output's current workspace.
    ///
    /// Returns `false` if the window is already known.
    pub fn add_window(&mut self, win: WindowId, output: OutputId) -> bool {
        let ws = self.current_workspace(&output);
        self.add_window_on(win, output, ws)
    }

    /// Add `win` on `output` at an explicit workspace (clamped into range).
    ///
    /// Returns `false` if the window is already known.
    pub fn add_window_on(&mut self, win: WindowId, output: OutputId, ws: usize) -> bool {
        if self.window_workspace.contains_key(&win) {
            return false;
        }
        let ws = ws.min(self.count - 1);
        if self.mode == WorkspaceMode::PerOutput {
            self.per_output_current.entry(output.clone()).or_insert(0);
        }
        self.windows.push(win.clone());
        self.window_output.insert(win.clone(), output);
        self.window_workspace.insert(win, ws);
        true
    }

    /// Move a known window to `ws`. Out-of-range targets and unknown windows are ignored.
    pub fn move_window(&mut self, win: WindowId, ws: usize) {
        if ws >= self.count {
            return;
        }
        if let Some(slot) = self.window_workspace.get_mut(&win) {
            *slot = ws;
        }
    }

    /// Remove a window entirely, returning it if it was known.
    pub fn remove_window(&mut self, win: &WindowId) -> Option<WindowId> {
        let index = self.windows.iter().position(|candidate| candidate == win)?;
        self.windows.remove(index);
        self.window_output.remove(win);
        self.window_workspace.remove(win);
        Some(win.clone())
    }

    /// Windows visible on `output`, topmost first.
    ///
    /// In [`WorkspaceMode::Global`] the output is ignored and every window on the one global
    /// workspace is visible. In [`WorkspaceMode::PerOutput`] only windows belonging to `output`
    /// and sitting on that output's current workspace are visible.
    pub fn windows_visible(&self, output: &OutputId) -> Vec<WindowId> {
        let current = self.current_workspace(output);
        self.windows
            .iter()
            .rev()
            .filter(|win| {
                if self.window_workspace.get(*win).copied().unwrap_or(0) != current {
                    return false;
                }
                match self.mode {
                    WorkspaceMode::Global => true,
                    WorkspaceMode::PerOutput => {
                        self.window_output.get(*win).is_some_and(|o| o == output)
                    }
                }
            })
            .cloned()
            .collect()
    }

    /// Handle an output going away: its windows are rehomed to the primary output at the primary's
    /// current workspace, and the output's per-output cursor is dropped.
    ///
    /// The primary itself is never removed — it is the rehome target — so removing it is a no-op.
    /// In [`WorkspaceMode::Global`] this only updates bookkeeping: visibility already ignores the
    /// output, so the visible set is unchanged, as it is in today's global model.
    pub fn output_removed(&mut self, output: &OutputId) {
        if output == &self.primary {
            return;
        }
        let target_ws = self.current_workspace(&self.primary);
        let primary = self.primary.clone();
        let rehome: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|win| self.window_output.get(*win).is_some_and(|o| o == output))
            .cloned()
            .collect();
        for win in rehome {
            self.window_output.insert(win.clone(), primary.clone());
            self.window_workspace.insert(win, target_ws);
        }
        self.per_output_current.remove(output);
    }

    /// The output a window belongs to, if it is known.
    pub fn window_output(&self, win: &WindowId) -> Option<&OutputId> {
        self.window_output.get(win)
    }

    /// The workspace a known window is on.
    pub fn window_workspace(&self, win: &WindowId) -> Option<usize> {
        self.window_workspace.get(win).copied()
    }

    /// Whether the window is known to the model.
    pub fn contains(&self, win: &WindowId) -> bool {
        self.window_workspace.contains_key(win)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> PerOutputWorkspaces<&'static str, u64> {
        PerOutputWorkspaces::global("eDP-1")
    }

    fn per_output() -> PerOutputWorkspaces<&'static str, u64> {
        PerOutputWorkspaces::per_output("eDP-1")
    }

    #[test]
    fn default_mode_is_global() {
        assert_eq!(WorkspaceMode::default(), WorkspaceMode::Global);
        assert_eq!(global().mode(), WorkspaceMode::Global);
        assert_eq!(per_output().mode(), WorkspaceMode::PerOutput);
    }

    #[test]
    fn global_mode_has_one_current_workspace_for_every_output() {
        let mut m = global();
        assert_eq!(m.current_workspace(&"eDP-1"), 0);
        assert_eq!(m.current_workspace(&"HDMI-1"), 0);
        assert!(m.switch("HDMI-1", 2));
        // The output argument is ignored: both outputs now read the single global workspace.
        assert_eq!(m.current_workspace(&"eDP-1"), 2);
        assert_eq!(m.current_workspace(&"HDMI-1"), 2);
        assert!(!m.switch("eDP-1", 2), "switching to current is a no-op");
    }

    #[test]
    fn global_visibility_ignores_the_output() {
        let mut m = global();
        assert!(m.add_window(1, "eDP-1"));
        assert!(m.add_window_on(2, "HDMI-1", 1));
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
        assert_eq!(m.windows_visible(&"HDMI-1"), vec![1]);
        m.switch("HDMI-1", 1);
        assert_eq!(m.windows_visible(&"eDP-1"), vec![2]);
        assert_eq!(m.windows_visible(&"HDMI-1"), vec![2]);
    }

    #[test]
    fn per_output_mode_tracks_a_current_workspace_per_output() {
        let mut m = per_output();
        assert!(m.switch("eDP-1", 1));
        assert_eq!(m.current_workspace(&"eDP-1"), 1);
        assert_eq!(
            m.current_workspace(&"HDMI-1"),
            0,
            "an output never seen reads as workspace 0"
        );
        assert!(m.switch("HDMI-1", 2));
        assert_eq!(m.current_workspace(&"eDP-1"), 1, "switching HDMI leaves eDP alone");
        assert_eq!(m.current_workspace(&"HDMI-1"), 2);
        assert!(!m.switch("HDMI-1", 2), "switching to its own current is a no-op");
    }

    #[test]
    fn per_output_visibility_is_scoped_to_output_and_workspace() {
        let mut m = per_output();
        assert!(m.add_window(1, "eDP-1"));
        assert!(m.add_window(2, "HDMI-1"));
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
        assert_eq!(m.windows_visible(&"HDMI-1"), vec![2]);

        m.switch("eDP-1", 1);
        assert!(m.windows_visible(&"eDP-1").is_empty());
        assert_eq!(m.windows_visible(&"HDMI-1"), vec![2], "other output unchanged");

        m.move_window(1, 1);
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
        m.move_window(1, 0);
        assert!(m.windows_visible(&"eDP-1").is_empty());
    }

    #[test]
    fn move_window_ignores_out_of_range_and_unknown_windows() {
        let mut m = global();
        assert!(m.add_window(1, "eDP-1"));
        m.move_window(1, m.workspace_count());
        assert_eq!(m.window_workspace(&1), Some(0));
        m.move_window(42, 1);
        assert_eq!(m.window_workspace(&42), None);
    }

    #[test]
    fn switch_rejects_out_of_range() {
        let mut m = global();
        assert!(!m.switch("eDP-1", m.workspace_count()));
        assert!(!m.switch("eDP-1", usize::MAX));
        assert_eq!(m.current_workspace(&"eDP-1"), 0);
    }

    #[test]
    fn visibility_is_topmost_first() {
        let mut m = global();
        m.add_window(1, "eDP-1");
        m.add_window(2, "eDP-1");
        m.add_window(3, "eDP-1");
        assert_eq!(m.windows_visible(&"eDP-1"), vec![3, 2, 1]);
    }

    #[test]
    fn adding_a_known_window_twice_is_rejected() {
        let mut m = per_output();
        assert!(m.add_window(1, "eDP-1"));
        assert!(!m.add_window(1, "HDMI-1"));
        assert_eq!(m.window_output(&1), Some(&"eDP-1"));
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
    }

    #[test]
    fn output_removed_rehomes_windows_to_primary() {
        let mut m = per_output();
        assert!(m.switch("eDP-1", 2));
        assert!(m.add_window(1, "HDMI-1"));
        assert!(m.add_window(2, "HDMI-1"));
        assert!(m.add_window(3, "eDP-1"));

        m.output_removed(&"HDMI-1");

        // Windows 1 and 2 now belong to the primary output at its current workspace (2).
        assert_eq!(m.window_output(&1), Some(&"eDP-1"));
        assert_eq!(m.window_output(&2), Some(&"eDP-1"));
        assert_eq!(m.window_workspace(&1), Some(2));
        assert_eq!(m.window_workspace(&2), Some(2));
        let visible = m.windows_visible(&"eDP-1");
        assert!(visible.contains(&1));
        assert!(visible.contains(&2));
        // The primary window is untouched.
        assert_eq!(m.window_output(&3), Some(&"eDP-1"));
        assert_eq!(m.window_workspace(&3), Some(2));
    }

    #[test]
    fn removing_the_primary_is_a_no_op() {
        let mut m = per_output();
        m.add_window(1, "eDP-1");
        m.output_removed(&"eDP-1");
        assert_eq!(m.window_output(&1), Some(&"eDP-1"));
        assert_eq!(m.window_workspace(&1), Some(0));
    }

    #[test]
    fn output_removed_in_global_mode_leaves_visibility_unchanged() {
        let mut m = global();
        m.add_window(1, "HDMI-1");
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
        m.output_removed(&"HDMI-1");
        assert_eq!(m.window_output(&1), Some(&"eDP-1"));
        assert_eq!(m.windows_visible(&"eDP-1"), vec![1]);
        assert_eq!(m.windows_visible(&"HDMI-1"), vec![1]);
    }

    #[test]
    fn remove_window_drops_all_bookkeeping() {
        let mut m = per_output();
        m.add_window(1, "eDP-1");
        assert_eq!(m.remove_window(&1), Some(1));
        assert!(!m.contains(&1));
        assert_eq!(m.window_output(&1), None);
        assert_eq!(m.window_workspace(&1), None);
        assert_eq!(m.remove_window(&1), None);
        assert!(m.windows_visible(&"eDP-1").is_empty());
    }

    #[test]
    fn workspace_count_is_clamped_like_the_global_model() {
        let m = PerOutputWorkspaces::<u32, u64>::new(WorkspaceMode::PerOutput, 0, 0);
        assert_eq!(m.workspace_count(), 1);
        let m = PerOutputWorkspaces::<u32, u64>::new(WorkspaceMode::Global, 999, 0);
        assert_eq!(m.workspace_count(), flamewm_api::workspace::MAX_WORKSPACES);
    }
}
