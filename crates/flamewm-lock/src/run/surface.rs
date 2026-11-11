//! Lock surface construction: overlay configs, shown-and-raised surfaces and per-surface background updates.

use super::*;

/// Config of the full-screen overlay surface shared by the lock and its previews.
pub(super) fn overlay_config(width: u32, height: u32, title: &str) -> SurfaceConfig {
    overlay_at(
        Rect::new(0, 0, width as i32, height as i32),
        title,
        SurfaceInputMode::Interactive,
    )
}

pub(super) fn overlay_at(rect: Rect, title: &str, input: SurfaceInputMode) -> SurfaceConfig {
    SurfaceConfig {
        width: rect.width.max(1) as u32,
        height: rect.height.max(1) as u32,
        title: title.to_string(),
        role: SurfaceRole::Overlay,
        input,
        initially_visible: false,
        x: rect.x,
        y: rect.y,
    }
}

/// Shows and raises `surface`; `what` names it in the error.
pub(super) fn show_raised(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    what: &str,
) -> Result<(), String> {
    runtime
        .show(surface)
        .map_err(|error| format!("show {what} surface: {error:?}"))?;
    runtime
        .raise(surface)
        .map_err(|error| format!("raise {what} surface: {error:?}"))
}

pub(super) fn update_animated_backgrounds(
    runtime: &mut SurfaceRuntime,
    surfaces: &[LockSurface],
    fallback: Option<&LockSurface>,
    frames: &mut [RibbonFrames],
    fallback_frames: Option<&mut RibbonFrames>,
    now: Instant,
) -> Result<(), String> {
    for (surface, frame) in surfaces.iter().zip(frames) {
        let image = frame.frame(now);
        runtime
            .with_document(surface.handle, |view| document::set_background(view, image))
            .map_err(|error| format!("update lock background: {error:?}"))?;
        runtime
            .redraw(surface.handle)
            .map_err(|error| format!("redraw lock surface: {error:?}"))?;
    }
    if let (Some(surface), Some(frame)) = (fallback, fallback_frames) {
        let image = frame.frame(now);
        runtime
            .with_document(surface.handle, |view| document::set_background(view, image))
            .map_err(|error| format!("update fallback lock background: {error:?}"))?;
        runtime
            .redraw(surface.handle)
            .map_err(|error| format!("redraw fallback lock surface: {error:?}"))?;
    }
    Ok(())
}

pub(super) fn restore_surface_backgrounds(
    runtime: &mut SurfaceRuntime,
    surfaces: &[LockSurface],
    fallback: Option<&LockSurface>,
) -> Result<(), String> {
    for surface in surfaces.iter().chain(fallback) {
        let base = surface.base_background.clone();
        runtime
            .with_document(surface.handle, |view| document::set_background(view, base))
            .map_err(|error| format!("restore lock background: {error:?}"))?;
    }
    Ok(())
}

pub(super) fn create_lock_surface(
    runtime: &mut SurfaceRuntime,
    settings: &LockSettings,
    root: Rect,
    canvas: Option<&RuntimeImage>,
    rect: Rect,
    title: &str,
) -> Result<LockSurface, String> {
    let template = document::template()?;
    let width = rect.width.max(1) as u32;
    let height = rect.height.max(1) as u32;
    let background = document::background_for_output(settings, root, rect, canvas);
    let handle = runtime
        .create_surface(
            template,
            overlay_at(rect, title, SurfaceInputMode::Interactive),
        )
        .map_err(|error| format!("create {title} surface: {error:?}"))?;
    runtime
        .with_document(handle, |view| {
            document::apply_document(view, settings, width, height, background.clone())
        })
        .map_err(|error| format!("theme {title} surface: {error:?}"))?;
    Ok(LockSurface {
        handle,
        rect,
        base_background: background,
    })
}
