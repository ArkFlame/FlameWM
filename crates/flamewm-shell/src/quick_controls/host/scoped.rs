use super::*;

/// Coalesce queued commands: consecutive OPENs collapse to the newest;
/// CLOSE/SHUTDOWN are best-effort singletons and never duplicate work.
pub(crate) fn coalesce_commands(queue: &mut VecDeque<Command>) -> VecDeque<Command> {
    let mut out: VecDeque<Command> = VecDeque::new();
    for cmd in queue.drain(..) {
        if matches!(&cmd, Command::Open(req) if req.scope.is_none()) {
            while matches!(out.back(), Some(Command::Open(req)) if req.scope.is_none()) {
                out.pop_back();
            }
        }
        out.push_back(cmd);
    }
    out
}

pub(crate) fn accept_scoped_open(
    latest_sequence: &mut u64,
    scope: Option<&super::super::protocol::OpenScope>,
) -> bool {
    let Some(scope) = scope else { return true };
    if scope.sequence <= *latest_sequence {
        return false;
    }
    *latest_sequence = scope.sequence;
    true
}

pub(crate) fn close_matches_active(active_sequence: Option<u64>, sequence: u64) -> bool {
    active_sequence == Some(sequence)
}

pub(crate) fn scoped_result(
    scope: Option<&OpenScope>,
    presented: bool,
    rolled_back: bool,
) -> Option<OpenResult> {
    let sequence = scope?.sequence;
    Some(if presented {
        OpenResult::Presented(sequence)
    } else if rolled_back {
        OpenResult::RolledBack(sequence)
    } else {
        OpenResult::Refused(sequence)
    })
}

pub(crate) fn send_open_result(result: Option<OpenResult>) {
    use std::io::Write;
    if let Some(result) = result {
        // The parent drains this pipe nonblocking once per shell turn. Frames
        // are tiny and sequence fenced; failure is recovered as host exit.
        let _ = std::io::stdout()
            .lock()
            .write_all(format!("{}\n", result.encode()).as_bytes());
    }
}

#[derive(Debug)]
pub(crate) struct OpenFailure {
    pub(crate) reason: String,
    pub(crate) rolled_back: bool,
}

impl OpenFailure {
    pub(crate) fn refused(reason: impl ToString) -> Self {
        Self {
            reason: reason.to_string(),
            rolled_back: false,
        }
    }
}

impl core::fmt::Display for OpenFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// Fetches the current settings over the control session for one popup open. A failed connect
/// or call keeps the last snapshot (nonfatal: the popup still opens with the previous values).
pub(crate) fn refresh_settings_for_open(state: &std::rc::Rc<std::cell::RefCell<HostState>>) {
    match ControlClient::connect(BusKind::Session) {
        Ok(client) => {
            // Windows exist once a popup opens, so the live cursor reload is safe only here.
            let mut host = state.borrow_mut();
            host.snapshot.refresh_settings(&client);
            let HostState {
                snapshot,
                cursor_theme,
                ..
            } = &mut *host;
            flamewm_ui_x11::apply_render_acceleration(&snapshot.settings);
            cursor_theme.sync_snapshot(&snapshot.settings);
        }
        Err(error) => eprintln!(
            "flamewm-quick-control: settings refresh skipped: {}",
            error.message
        ),
    }
}
