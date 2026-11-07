use super::*;

/// Reports one connector that was left out, as `stage=outputs status=skip detail=<connector> <why>`.
///
/// A connector that cannot be set up never aborts the session: it is skipped here and the remaining
/// connectors still come up.
pub(super) fn skip_connector(device: &DrmDevice, conn: connector::Handle, reason: &str) {
    let name = match device.get_connector(conn, false) {
        Ok(info) => format!("{}-{}", info.interface().as_str(), info.interface_id()),
        Err(_) => "unknown".to_owned(),
    };
    stage_line(
        Stage::Outputs,
        StageStatus::Skip,
        &format!("{name} {reason}"),
    );
}

/// Adds connectors that became connected and drops outputs whose connector went away.
///
/// Returns an error only for a device-level failure (the connector list cannot be read at all). A
/// single connector that fails to come up is logged with `status=skip` and skipped; startup decides
/// with `state::scan_failed` whether nothing at all came up.
pub(super) fn scan_connectors(d: &mut Drm, state: &mut State) -> Result<(), Box<dyn Error>> {
    let resources = d.device.resource_handles()?;

    let mut index = 0;
    while index < d.surfaces.len() {
        let connector = d.surfaces[index].connector;
        let connected = d
            .device
            .get_connector(connector, false)
            .map(|info| info.state() == connector::State::Connected)
            .unwrap_or(false);
        if connected {
            index += 1;
            continue;
        }
        let gone = d.surfaces.remove(index);
        if let Some(global) = gone.global {
            state.display.remove_global::<State>(global);
        }
        eprintln!(
            "flamewm-wayland: output {} disconnected",
            gone.output.name()
        );
    }

    for conn in resources.connectors().to_vec() {
        if d.surfaces.iter().any(|out| out.connector == conn) {
            continue;
        }
        // A forced probe reads EDID and can stall for seconds on a flaky external connector, which
        // delays the panel behind it. The cached state is enough unless it reports a connected
        // connector with no modes.
        let mut info = match d.device.get_connector(conn, false) {
            Ok(info) => info,
            Err(error) => {
                skip_connector(&d.device, conn, &format!("read {error}"));
                continue;
            }
        };
        if info.state() == connector::State::Connected && info.modes().is_empty() {
            match d.device.get_connector(conn, true) {
                Ok(forced) => info = forced,
                Err(error) => {
                    skip_connector(&d.device, conn, &format!("read {error}"));
                    continue;
                }
            }
        }
        if info.state() != connector::State::Connected {
            continue;
        }
        let used: Vec<crtc::Handle> = d.surfaces.iter().map(|out| out.crtc).collect();
        let Some(crtc) = pick_crtc(&d.device, &resources, &info, &used) else {
            skip_connector(&d.device, conn, "no-crtc");
            continue;
        };
        // Preferred resolution at its highest refresh rate; see `output_layout::select_mode`.
        let candidates: Vec<ModeCandidate> = info
            .modes()
            .iter()
            .map(|mode| ModeCandidate {
                width: i32::from(mode.size().0),
                height: i32::from(mode.size().1),
                refresh_mhz: i32::try_from(mode.vrefresh()).unwrap_or(60) * 1000,
                preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
            })
            .collect();
        let Some(mode) = select_mode(&candidates).map(|index| info.modes()[index]) else {
            skip_connector(&d.device, conn, "no-mode");
            continue;
        };
        // Two attempts, like anvil: a KMS state left behind by whoever owned this connector before
        // us is the usual reason a surface cannot be created, and one `DrmDevice::reset_state`
        // clears it. A connector that still fails is skipped, never fatal.
        let mut attempt = 0;
        loop {
            match build_output(d, state, conn, crtc, mode, &info) {
                Ok(out) => {
                    eprintln!(
                        "flamewm-wayland: output {} {}x{}",
                        out.output.name(),
                        i32::from(mode.size().0),
                        i32::from(mode.size().1)
                    );
                    d.surfaces.push(out);
                    break;
                }
                Err(error) => {
                    if attempt > 0 {
                        skip_connector(&d.device, conn, &format!("setup {error}"));
                        break;
                    }
                    attempt += 1;
                    eprintln!("flamewm-wayland: output setup retry detail={error}");
                    if let Err(error) = d.device.reset_state() {
                        skip_connector(&d.device, conn, &format!("reset {error}"));
                        break;
                    }
                    // A device reset disables every connector and plane, so the outputs that are
                    // already up have to be painted again in full.
                    for out in &mut d.surfaces {
                        out.frames.wake();
                        if let Err(error) = out.compositor.reset_state() {
                            eprintln!(
                                "flamewm-wayland: reset failed on {}: {error}",
                                out.output.name()
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// True when the DRM driver identifies as NVIDIA; anvil disables overlay planes for that driver
/// because using them breaks the atomic commit and the first frame never reaches the panel.
pub(super) fn is_nvidia(device: &DrmDevice) -> bool {
    device
        .get_driver()
        .map(|driver| {
            let name = driver.name().to_string_lossy().to_lowercase();
            let description = driver.description().to_string_lossy().to_lowercase();
            name.contains("nvidia") || description.contains("nvidia")
        })
        .unwrap_or(false)
}

/// The first CRTC one of this connector's encoders can drive that no output uses yet.
pub(super) fn pick_crtc(
    device: &DrmDevice,
    resources: &ResourceHandles,
    info: &connector::Info,
    used: &[crtc::Handle],
) -> Option<crtc::Handle> {
    for encoder in info.encoders() {
        let Ok(encoder) = device.get_encoder(*encoder) else {
            continue;
        };
        let mut candidates: Vec<crtc::Handle> = encoder.crtc().into_iter().collect();
        candidates.extend(resources.filter_crtcs(encoder.possible_crtcs()));
        if let Some(crtc) = candidates.into_iter().find(|crtc| !used.contains(crtc)) {
            return Some(crtc);
        }
    }
    None
}

/// Creates the scanout surface, the wayland `Output` (or the shared primary one) and the
/// `DrmCompositor` for one connector.
pub(super) fn build_output(
    d: &mut Drm,
    state: &mut State,
    conn: connector::Handle,
    crtc: crtc::Handle,
    mode: DrmMode,
    info: &connector::Info,
) -> Result<OutputSurface, Box<dyn Error>> {
    eprintln!("flamewm-wayland: init phase=create_surface");
    let surface = d.device.create_surface(crtc, mode, &[conn])?;
    let (width, height) = mode.size();
    let mode = Mode {
        size: (i32::from(width), i32::from(height)).into(),
        refresh: i32::try_from(mode.vrefresh()).unwrap_or(60) * 1000,
    };
    // The first output is `State::output`, which already has a global; later ones get their own.
    let (output, global) = if d.surfaces.iter().any(|out| out.global.is_none()) {
        let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let size_mm = info
            .size()
            .map(|(w, h)| Size::from((w as i32, h as i32)))
            .unwrap_or_default();
        let output = Output::new(
            name,
            PhysicalProperties {
                size: size_mm,
                subpixel: Subpixel::Unknown,
                make: "FlameWM".to_owned(),
                model: "DRM".to_owned(),
            },
        );
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            None,
        );
        output.set_preferred(mode);
        let global = output.create_global::<State>(&state.display);
        (output, Some(global))
    } else {
        state.output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        state.output.set_preferred(mode);
        state.output_size = Size::from((mode.size.w, mode.size.h));
        (state.output.clone(), None)
    };
    let allocator = GbmAllocator::new(
        d.gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(d.gbm.clone(), None);
    // Anvil disables overlay planes on NVIDIA: using them there breaks the atomic commit and the
    // first frame never reaches the panel. Dropping them costs one extra composition, not correctness.
    let mut planes = surface.planes().clone();
    if is_nvidia(&d.device) {
        planes.overlay.clear();
    }
    eprintln!("flamewm-wayland: init phase=compositor");
    let compositor = match DrmCompositor::new(
        &output,
        surface,
        Some(planes),
        allocator,
        exporter,
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        d.formats.iter().copied(),
        d.device.cursor_size(),
        Some(d.gbm.clone()),
    ) {
        Ok(compositor) => compositor,
        Err(error) => {
            // The global belongs to an output that will never appear: withdraw it before the
            // caller retries, so a retry does not leave a second one behind.
            if let Some(global) = global {
                state.display.remove_global::<State>(global);
            }
            return Err(error.into());
        }
    };
    Ok(OutputSurface {
        connector: conn,
        crtc,
        output,
        global,
        compositor,
        // A connector that just appeared has never been painted.
        frames: OutputFrames {
            dirty: true,
            ..OutputFrames::default()
        },
    })
}

/// Drops every output surface and its compositor. The connectors go back to `State`, which keeps
/// the primary `Output` global alive.
pub(super) fn drop_outputs(d: &mut Drm, state: &mut State) {
    for out in d.surfaces.drain(..) {
        if let Some(global) = out.global {
            state.display.remove_global::<State>(global);
        }
    }
}
