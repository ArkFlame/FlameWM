use super::*;

pub(crate) fn open_request(
    runtime: &mut SurfaceRuntime,
    surfaces: &mut HelperSurfaces,
    snapshot: &QuickControlSnapshot,
    request: &OpenRequest,
    close_elapsed: Duration,
) -> Result<(), OpenFailure> {
    let total_start = std::time::Instant::now();
    let _total = crate::runtime::shell_span("shell.quick.open.total").start();
    flamewm_debug::emit(POPUP_TRANSITION, Duration::from_millis(0), || {
        format!(
            "quick-control open kind={:?} anchor={:?} work_area={:?} panel_edge={:?}",
            request.kind, request.anchor, request.work_area, request.panel_edge,
        )
    });
    let surface = surfaces
        .ensure(runtime, request.kind)
        .map_err(OpenFailure::refused)?;
    let project_start = std::time::Instant::now();
    let _project = crate::runtime::shell_span("shell.quick.project").start();
    project_snapshot(runtime, surface, snapshot, request.kind).map_err(OpenFailure::refused)?;
    let project_elapsed = project_start.elapsed();
    drop(_project);
    let (rect, measure_elapsed, place_elapsed) =
        fitted_rect(runtime, surface, request).map_err(OpenFailure::refused)?;
    flamewm_debug::emit(POPUP_TRANSITION, Duration::from_millis(0), || {
        format!("quick-control fitted kind={:?} rect={rect:?}", request.kind)
    });
    let prepare_start = std::time::Instant::now();
    let _prepare = crate::runtime::shell_span("shell.quick.prepare/commit_geometry").start();
    prepare_commit(runtime, surface, rect).map_err(OpenFailure::refused)?;
    let prepare_elapsed = prepare_start.elapsed();
    drop(_prepare);
    // Same frosted backdrop as the Start menu: the desktop layer, installed at the first show.
    if let Some(root) = PACKAGED_ROOT.get() {
        runtime.set_desktop_source(Some(crate::surface_backdrop::desktop_source(
            &snapshot.settings,
            root,
        )));
    }
    let backdrop = crate::settings_projection::start_backdrop_percents(&snapshot.settings);
    if let Err(error) = runtime.set_popup_backdrop(surface, Some(backdrop)) {
        eprintln!("flamewm-quick-control: popup backdrop skipped (nonfatal): {error:?}");
    }
    // This helper has no Dock surface: the parent's panel rect clips slide transit frames.
    runtime.set_external_dock(request.dock.map(|dock| {
        (
            dock.x,
            dock.y,
            dock.width.max(0) as u32,
            dock.height.max(0) as u32,
        )
    }));
    // Same Start slide preference and speed as the Start menu, on this panel edge.
    let slide = popup_slide(&snapshot.settings, request.panel_edge);
    let slide = match arm_slide(runtime, surface, slide) {
        Ok(()) => slide,
        Err(error) => {
            eprintln!("flamewm-quick-control: popup slide skipped (nonfatal): {error}");
            None
        }
    };
    let present_start = std::time::Instant::now();
    let _present = crate::runtime::shell_span("shell.quick.present").start();
    present(runtime, surface).map_err(OpenFailure::refused)?;
    let present_elapsed = present_start.elapsed();
    drop(_present);
    // DEBUG-ONLY: one observed root-rect probe after present. Normal mode
    // issues zero extra X sync.
    let trace = flamewm_ui_x11::GeometryTrace::begin(request.kind.as_str());
    if host_debug_enabled() {
        let _ = runtime.debug_probe_root_rect(surface, trace.for_surface(0));
    }
    let observed_rect = runtime
        .surface_device_rect(surface)
        .ok()
        .map(|r| (r.x as i32, r.y as i32, r.width as i32, r.height as i32));
    let grab_start = std::time::Instant::now();
    let _grab = crate::runtime::shell_span("shell.quick.pointer_grab").start();
    let grab_result = grab(runtime, surface).map_err(|error| error.to_string());
    if let Err(error) = grab_result {
        drop(_grab);
        let rolled_back = match close_helper(runtime, surface, None) {
            Ok(()) => true,
            Err(close_error) => {
                eprintln!("flamewm-quick-control: popup rollback close failed: {close_error}");
                false
            }
        };
        match surfaces.open {
            Some(kind) if kind != request.kind => {}
            _ => surfaces.open = None,
        }
        return Err(OpenFailure {
            reason: error,
            rolled_back,
        });
    }
    let grab_elapsed = grab_start.elapsed();
    drop(_grab);
    surfaces.open = Some(request.kind);
    surfaces.slide = slide;
    let total = total_start.elapsed();
    if host_debug_enabled() {
        eprintln!(
            "popup-record txn={} kind={:?} panel_revision={} edge={:?} panel_rect={:?} output_rect={:?} work_area={:?} anchor={:?} intrinsic=n/a fitted={:?} native_request={:?} native_observed={:?}",
            trace.txn,
            request.kind,
            snapshot.system.revision,
            request.panel_edge,
            request.anchor,
            request.work_area,
            request.work_area,
            request.anchor,
            rect,
            rect,
            observed_rect,
        );
        match runtime.debug_stack_canary() {
            Ok(true) => {}
            Ok(false) => {
                let _ = runtime.raise(surface);
                eprintln!(
                    "flamewm-quick-control: debug stack canary FAILED kind={:?}; re-raised via existing owner",
                    request.kind
                );
            }
            Err(error) => {
                eprintln!("flamewm-quick-control: debug stack canary error: {error:?}");
            }
        }
    }
    if is_slow_open(total) {
        eprintln!(
            "{}",
            OpenStageTimings {
                total,
                project: project_elapsed,
                measure: measure_elapsed,
                place: place_elapsed,
                prepare_commit: prepare_elapsed,
                present: present_elapsed,
                pointer_grab: grab_elapsed,
                close: close_elapsed,
            }
            .render(request.kind)
        );
    }
    Ok(())
}
