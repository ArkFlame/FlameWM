use super::*;

/// What a change of the window's fully-obscured state asks the widget loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityEffect {
    Suspend,
    Resume,
}

/// Gate for `set_obscured`: `None` when the state did not change.
pub(super) fn visibility_effect(was_obscured: bool, obscured: bool) -> Option<VisibilityEffect> {
    match (was_obscured, obscured) {
        (false, true) => Some(VisibilityEffect::Suspend),
        (true, false) => Some(VisibilityEffect::Resume),
        _ => None,
    }
}

/// Applies a `VisibilityNotify` verdict. While obscured, no sampling and no document sync run and
/// the wake deadline is dropped, so the reactor does not spin on a past deadline. On uncover the
/// cached sync key is discarded: writes made while covered bypass the key, so the resume sync
/// must compare the document again. It then samples and syncs immediately.
pub(crate) fn set_obscured(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    obscured: bool,
) -> Result<(), String> {
    match visibility_effect(state.widgets.obscured, obscured) {
        None => Ok(()),
        Some(VisibilityEffect::Suspend) => {
            state.widgets.obscured = true;
            state.widgets.wake.set(None);
            Ok(())
        }
        Some(VisibilityEffect::Resume) => {
            state.widgets.obscured = false;
            state.widgets.sync_key = None;
            state.widgets.sample();
            state.widgets.arm(Instant::now());
            sync_widgets(document, state)
        }
    }
}

pub(crate) fn poll(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if state.widgets.obscured {
        state.widgets.wake.set(None);
        return Ok(());
    }
    let now = Instant::now();
    let sampling = state.widgets.sample_due.is_some_and(|due| now >= due);
    let ticking = state.widgets.due.is_some_and(|due| now >= due);
    let media = std::mem::take(&mut state.widgets.media_dirty);
    if !sampling && !ticking && !media {
        return Ok(());
    }
    if sampling {
        state.widgets.sample();
    }
    if sampling || ticking {
        state.widgets.arm(now);
    }
    sync_widgets(document, state)
}

pub(crate) fn refresh_after_change(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    state.widgets.sample();
    state.widgets.arm(Instant::now());
    state.widgets.persist();
    sync_widgets(document, state)
}

pub(crate) fn refresh_media(state: &mut DesktopState) {
    let Some(provider) = state.widgets.mpris.as_mut() else {
        return;
    };
    if let Err(error) = provider.on_ready() {
        super::super::log_limit::emit(
            "media widget update failed",
            format_args!("desktop: media widget update failed: {}", error.message),
        );
    }
    provider.reconcile();
    let snapshot = provider.snapshot();
    state.widgets.set_media(snapshot);
}

#[cfg(test)]
mod visibility_gate_tests {
    use super::{VisibilityEffect, visibility_effect};

    #[test]
    fn gate_suspends_only_on_obscure_and_resumes_only_on_uncover() {
        assert_eq!(
            visibility_effect(false, true),
            Some(VisibilityEffect::Suspend)
        );
        assert_eq!(
            visibility_effect(true, false),
            Some(VisibilityEffect::Resume)
        );
        assert_eq!(visibility_effect(false, false), None);
        assert_eq!(visibility_effect(true, true), None);
    }
}
