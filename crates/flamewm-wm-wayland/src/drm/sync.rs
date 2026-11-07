use super::*;

/// Returns the decision and the new cumulative wait.
pub(super) fn frame_sync_wait<B, F, E>(
    result: &RenderFrameResult<'_, B, F, E>,
    already_waited: u128,
) -> (SyncWait, u128)
where
    B: Buffer,
    F: Framebuffer,
{
    if !result.needs_sync() {
        // KMS waits for our rendering itself: nothing to synchronize, the frame may be queued.
        return (SyncWait::Wait, 0);
    }
    let Some(element) = (match &result.primary_element {
        PrimaryPlaneElement::Swapchain(element) => Some(element),
        // Direct scan-out of a client buffer: no composited buffer of ours is in flight.
        PrimaryPlaneElement::Element(_) => None,
    }) else {
        return (SyncWait::Wait, 0);
    };
    let sync: &SyncPoint = &element.sync;
    if sync.is_reached() {
        // The fence already signaled: the wait below would not block at all.
        return (SyncWait::Wait, 0);
    }
    let Some(fence) = sync.get::<EGLFence>() else {
        // A fence type without a timeout API. Never block the compositor thread on it: report and let
        // the output be retried. Smithay's GLES renderer only produces an `EGLFence` or an already
        // signaled point (`.../renderer/gles/mod.rs:2328-2345`), so this is a safety net.
        return (SyncWait::Fail, already_waited);
    };
    let start = Instant::now();
    // One bounded kernel wait, not a spin: `eglClientWaitSyncKHR` returns as soon as the fence
    // signals, and at the latest after the budget.
    let signaled = fence
        .client_wait(Some(SYNC_WAIT_BUDGET), false)
        .unwrap_or(false);
    let waited = already_waited.saturating_add(start.elapsed().as_millis());
    if signaled {
        return (SyncWait::Wait, 0);
    }
    (sync_wait_decision(true, waited), waited)
}

/// Whether a render error is the kernel rejecting the atomic state (`DrmError::TestFailed`).
///
/// The error type of `DrmCompositor::render_frame` cannot be named outside smithay, but the pattern
/// is public, so the check is a pattern match on the outer error. Its source chain is checked too,
/// because a `PrepareFrame` wraps the `FrameError` that carries the DRM error.
pub(super) fn is_test_failed<A, B, F, R>(error: &RenderFrameError<A, B, F, R>) -> bool
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: std::error::Error,
{
    match error {
        RenderFrameError::PrepareFrame(FrameError::DrmError(DrmError::TestFailed(_))) => true,
        _ => matches!(error.source(), Some(source)
            if matches!(source.downcast_ref::<DrmError>(), Some(DrmError::TestFailed(_)))),
    }
}
