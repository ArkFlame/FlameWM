//! Settings preview loop: shows the lock UI or the Ribbons screensaver without grabs or authentication.

use super::*;
use crate::preview;

/// The lock projection for a preview, built only for the non-Ribbons preview. Ribbons
/// shows no lock UI, so `project` is never called for it.
pub(super) fn preview_lock_ui<T>(ribbons: bool, project: impl FnOnce() -> T) -> Option<T> {
    (!ribbons).then(project)
}

/// Shared preview loop. `ribbons` selects the animated background and nothing else.
pub(super) fn preview_loop(
    settings: &LockSettings,
    username: &str,
    ribbons: bool,
) -> Result<(), String> {
    let (width, height) = flamewm_ui_x11::root_geometry()?;
    let template = document::template()?;
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    let title = if ribbons {
        "FlameWM screensaver preview"
    } else {
        "FlameWM lock preview"
    };
    let surface = runtime
        .create_surface(template, overlay_config(width, height, title))
        .map_err(|error| format!("create preview surface: {error:?}"))?;
    let started = Instant::now();
    let mut frames = ribbons.then(|| RibbonFrames::new(width, height, &settings.saver, started));
    let background = match frames.as_mut() {
        Some(frames) => frames.frame(started),
        None => document::background_image(settings, width, height),
    };
    let mut next_frame = started + FRAME_INTERVAL;
    // Ribbons shows the background alone: its lock projection is never built.
    let lock_ui = preview_lock_ui(ribbons, || {
        (
            LockView::project(
                settings,
                local_now(),
                username,
                &PasswordBuffer::default(),
                Status::default(),
            ),
            PowerUi::new(SessionCapabilities::default()).view(),
        )
    });
    runtime
        .with_document(surface, |document_view| {
            document::apply_document(document_view, settings, width, height, background)?;
            match &lock_ui {
                Some((view, power)) => {
                    document::apply_view(document_view, view)?;
                    document::apply_power(document_view, width, height, power)
                }
                None => document::set_lock_ui_shown(document_view, false),
            }
        })
        .map_err(|error| format!("theme preview surface: {error:?}"))?;
    show_raised(&mut runtime, surface, "preview")?;
    let mapped = Instant::now();

    // Input is read by polling, so no grab is needed. The timer bounds each wait.
    let session = Session::open()?;
    signals::install_preview_terminate();
    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    reactor
        .register_timer(PREVIEW_POLL, true, || {})
        .map_err(|error| format!("register preview tick: {error}"))?;
    let sample = |session: &Session| {
        session
            .pointer()
            .map(|pointer| PreviewInput::new(pointer.x, pointer.y, pointer.mask, session.keymap()))
    };
    let mut previous = sample(&session);
    run_surface_runtime_with_reactor_access(
        &mut runtime,
        &mut reactor,
        |_, _| Ok(()),
        |runtime| {
            let current = sample(&session);
            let dismiss = signals::preview_terminate_requested()
                || matches!((previous, current),
                    (Some(before), Some(now)) if preview::dismisses(mapped.elapsed(), &before, &now));
            if dismiss {
                frames = None;
                runtime
                    .destroy(surface)
                    .map_err(|error| UiBackendError::Renderer(format!("destroy preview: {error:?}")))?;
            }
            if let Some(frames) = frames.as_mut() {
                let now = Instant::now();
                if now >= next_frame {
                    let image = frames.frame(now);
                    next_frame = now + FRAME_INTERVAL;
                    runtime
                        .with_document(surface, |view| document::set_background(view, image))
                        .map_err(|error| UiBackendError::Renderer(format!("update preview background: {error:?}")))?;
                    runtime
                        .redraw(surface)
                        .map_err(|error| UiBackendError::Renderer(format!("redraw preview: {error:?}")))?;
                }
            }
            previous = current;
            Ok(())
        },
    )
    .map_err(|error| format!("preview loop: {error:?}"))
}
