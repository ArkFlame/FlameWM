//! Quick-control popup kind mapping and open-result resolution.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuickOpenResolution {
    Presented,
    Failed,
    Stale,
}

pub(super) fn apply_quick_open_result(
    session: &mut flamewm_shell_core::popup_session::PopupSession,
    result: OpenResult,
) -> QuickOpenResolution {
    let (sequence, presented) = match result {
        OpenResult::Presented(sequence) => (sequence, true),
        OpenResult::Refused(sequence)
        | OpenResult::RolledBack(sequence)
        | OpenResult::HostExited(sequence) => (sequence, false),
    };
    let Some(lease) = session.active().filter(|lease| lease.sequence == sequence) else {
        return QuickOpenResolution::Stale;
    };
    if !is_quick_popup_kind(&lease.kind) {
        return QuickOpenResolution::Stale;
    }
    if presented {
        QuickOpenResolution::Presented
    } else {
        let close = session.close(sequence);
        session.commit(&close);
        QuickOpenResolution::Failed
    }
}

pub(super) fn quick_popup_kind(
    kind: flamewm_shell::quick_controls::QuickControlKind,
) -> flamewm_shell_core::popup_session::PopupKind {
    use flamewm_shell::quick_controls::QuickControlKind as Quick;
    use flamewm_shell_core::popup_session::PopupKind as Popup;
    match kind {
        Quick::Audio => Popup::Audio,
        Quick::Network => Popup::Network,
        Quick::Calendar => Popup::Calendar,
        Quick::Battery => Popup::Battery,
        Quick::Weather => Popup::Weather,
    }
}

pub(super) fn is_quick_popup_kind(kind: &flamewm_shell_core::popup_session::PopupKind) -> bool {
    use flamewm_shell_core::popup_session::PopupKind;
    matches!(
        kind,
        PopupKind::Audio
            | PopupKind::Network
            | PopupKind::Calendar
            | PopupKind::Battery
            | PopupKind::Weather
    )
}
