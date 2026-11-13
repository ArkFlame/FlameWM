//! Applies queued engine `Command`s to the compositor state after a bus turn.

use super::*;

pub(super) fn apply_command(state: &mut State, command: Command) {
    match command {
        Command::Activate(id) => {
            state.model.reveal(id);
            state.apply_geometry(id);
            state.set_focus(Some(id));
        }
        Command::Minimize(id) => {
            state.model.minimize(id);
            state.apply_geometry(id);
            if state.focused == Some(id) {
                state.refocus();
            }
        }
        Command::Restore(id) => {
            state.model.restore(id);
            state.apply_geometry(id);
            state.set_focus(Some(id));
        }
        Command::Maximize(id) => {
            state.refresh_dock_struts();
            let work = state.work_area_of(id);
            state.model.toggle_maximize(id, work);
            state.apply_geometry(id);
        }
        Command::Close(id) => state.close_window(id),
        Command::Spawn(argv) => {
            if let Some((program, args)) = argv.split_first() {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                state.spawn(program, &args);
            }
        }
        Command::SwitchWorkspace(index) => {
            if state.model.switch_workspace(index) {
                state.refocus();
            }
        }
        Command::MoveToWorkspace(id, target) => {
            state.model.move_to_workspace(id, target);
            state.apply_geometry(id);
            state.refocus();
        }
        Command::SetGeometry(id, rect) => {
            state.model.set_rect(id, rect);
            state.apply_geometry(id);
        }
        Command::SetShortcuts(bindings) => {
            state.shortcuts = crate::model::Shortcuts::from_bindings(&bindings);
        }
        Command::ApplyWorkspacePlan(plan) => {
            if state.model.apply_workspace_plan(&plan) {
                state.refocus();
            }
            state.request_redraw();
        }
        Command::SwitchWithWindow { id, target } => {
            // Both halves move in one turn with nothing rendered between them, so the window and
            // the visible workspace never disagree.
            state.model.move_to_workspace(id, target);
            state.model.switch_workspace(target);
            state.apply_geometry(id);
            state.refocus();
        }
        // Stops the loop; the session wrapper reaps the compositor and tears the session down.
        Command::Logout => state.loop_signal.stop(),
    }
    state.request_redraw();
}
