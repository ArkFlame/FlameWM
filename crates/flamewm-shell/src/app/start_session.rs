//! Start-menu toggle decisions and launch-session matching.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartToggleAction {
    Open,
    Close,
    Transfer,
}

pub(super) fn start_toggle_action(
    is_open: bool,
    open_output: Option<&flamewm_api::OutputId>,
    target_output: &flamewm_api::OutputId,
) -> StartToggleAction {
    if !is_open {
        StartToggleAction::Open
    } else if open_output == Some(target_output) {
        StartToggleAction::Close
    } else {
        StartToggleAction::Transfer
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartLaunchContext {
    pub(super) invocation: u64,
    pub(super) output: flamewm_api::OutputId,
    pub(super) popup_sequence: u64,
}

pub(super) fn start_launch_session_matches(
    context: &StartLaunchContext,
    start_open: bool,
    active: Option<&flamewm_shell_core::popup_session::PopupLease>,
) -> bool {
    start_open
        && active.is_some_and(|lease| {
            lease.kind == flamewm_shell_core::popup_session::PopupKind::Start
                && lease.output == context.output
                && lease.sequence == context.popup_sequence
        })
}
