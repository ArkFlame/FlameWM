//! Show-desktop strip: minimizes every visible window, then restores exactly that set.

use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::WindowRef;
use flamewm_control_core::ControlRequest;

/// Requests for one strip click, plus the new saved set. Visible windows are minimized and
/// remembered; with none visible, the remembered windows that are still minimized are restored.
#[must_use]
pub fn plan(
    windows: &[WindowSnapshot],
    saved: &[WindowRef],
) -> (Vec<ControlRequest>, Vec<WindowRef>) {
    let visible: Vec<WindowRef> = windows
        .iter()
        .filter(|window| window.is_visible())
        .map(|window| window.reference)
        .collect();
    if !visible.is_empty() {
        let requests = visible
            .iter()
            .copied()
            .map(ControlRequest::MinimizeWindow)
            .collect();
        return (requests, visible);
    }
    let requests = windows
        .iter()
        .filter(|window| window.state == WindowState::Minimized)
        .filter(|window| saved.iter().any(|s| s.id == window.reference.id))
        .map(|window| ControlRequest::RestoreWindow(window.reference))
        .collect();
    (requests, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::{DesktopAppId, OutputId, Rect, WorkspaceRef};

    fn window(id: u64, state: WindowState) -> WindowSnapshot {
        WindowSnapshot {
            reference: WindowRef::new(id, 1),
            title: String::new(),
            app_id: DesktopAppId::new("app"),
            outer_geometry: Rect::new(0, 0, 10, 10),
            restore_geometry: Rect::new(0, 0, 10, 10),
            state,
            sticky: false,
            focused: false,
            workspace: WorkspaceRef::new(0, 1),
            output: OutputId::new("A"),
            state_generation: 1,
        }
    }

    #[test]
    fn minimizes_visible_then_restores_only_that_set() {
        let mut windows = vec![
            window(1, WindowState::Normal),
            window(2, WindowState::Minimized),
        ];
        let (requests, saved) = plan(&windows, &[]);
        assert_eq!(
            requests,
            vec![ControlRequest::MinimizeWindow(WindowRef::new(1, 1))]
        );
        windows[0].state = WindowState::Minimized;
        let (requests, saved) = plan(&windows, &saved);
        assert_eq!(
            requests,
            vec![ControlRequest::RestoreWindow(WindowRef::new(1, 1))]
        );
        assert!(saved.is_empty());
    }
}
