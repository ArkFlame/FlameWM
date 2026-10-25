//! Seat GPU selection, DRM device ownership and the descriptor helpers that go with it.

use super::*;

/// One seat GPU that opened, with the descriptor kept for GBM.
pub(super) struct OpenedGpu {
    pub(super) name: String,
    /// `dev_t` of the card: udev change and resume events are matched against it.
    pub(super) dev: u64,
    pub(super) device: DrmDevice,
    pub(super) fd: DrmDeviceFd,
    pub(super) notifier: DrmDeviceNotifier,
}

/// Opens the seat's GPUs and picks the one to drive, plus a short detail token for the stage line.
///
/// The boot GPU is tried first and wins as soon as it reports a connected connector: that is the
/// common single-GPU and hybrid-desktop case and costs exactly one `TakeDevice` roundtrip. Only when
/// it has nothing connected are the seat's other GPUs probed — a hybrid laptop whose only panel
/// hangs off the non-boot card reports zero connectors on the boot GPU, which used to mean zero
/// connectors for the whole session. The choice itself is `state::pick_gpu`, unit-tested without a
/// GPU.
pub(super) fn open_scanout_gpu(
    session: &LogindSession,
    seat: &str,
) -> Result<(OpenedGpu, String), Box<dyn Error>> {
    let boot = udev::primary_gpu(seat)?;
    let mut paths: Vec<PathBuf> = boot.iter().cloned().collect();
    for path in udev::all_gpus(seat)? {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err("no DRM device on seat".into());
    }

    let mut opened: Vec<(OpenedGpu, GpuProbe)> = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match open_gpu(session, path) {
            Ok(gpu) => {
                let probe = probe_gpu(&gpu.device);
                // Only the boot GPU short-circuits: on any other card the rest of the seat is still
                // worth looking at, because a panel is what is being looked for.
                let boot_gpu = index == 0 && boot.is_some();
                opened.push((gpu, probe));
                if boot_gpu && probe.connected {
                    break;
                }
            }
            Err(error) => eprintln!("flamewm-wayland: gpu {name} skip: {error}"),
        }
    }

    let probes: Vec<GpuProbe> = opened.iter().map(|(_, probe)| *probe).collect();
    let chosen = pick_gpu(&probes).ok_or("no DRM device on seat")?;
    let detail = if probes[chosen].internal {
        "detail=panel"
    } else if probes[chosen].connected {
        "detail=connected"
    } else {
        "detail=none-connected"
    };
    let (gpu, _) = opened.swap_remove(chosen);
    // The cards that did not win are dropped here: their descriptors close and their master is
    // released before the compositor starts.
    Ok((gpu, detail.to_owned()))
}

/// Takes one DRM device from the session and opens it.
fn open_gpu(session: &LogindSession, path: &Path) -> Result<OpenedGpu, Box<dyn Error>> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dev = std::fs::metadata(path)?.rdev();
    let fd = session.clone().open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok(OpenedGpu {
        name,
        dev,
        device,
        fd,
        notifier,
    })
}

/// What one card can drive, without setting anything up: internal (panel) connectors and all
/// connected connectors.
fn probe_gpu(device: &DrmDevice) -> GpuProbe {
    let Ok(resources) = device.resource_handles() else {
        return GpuProbe::default();
    };
    let mut probe = GpuProbe::default();
    for conn in resources.connectors() {
        let Ok(info) = device.get_connector(*conn, false) else {
            continue;
        };
        if info.state() != connector::State::Connected {
            continue;
        }
        probe.connected = true;
        if matches!(
            info.interface(),
            connector::Interface::LVDS
                | connector::Interface::EmbeddedDisplayPort
                | connector::Interface::DSI
                | connector::Interface::DPI
        ) {
            probe.internal = true;
        }
    }
    probe
}

/// `fstat` of a descriptor logind handed over, as the `dev_t` to compare with the primary GPU.
pub(super) fn device_rdev(fd: &OwnedFd) -> std::io::Result<u64> {
    let probe = fd.try_clone()?;
    Ok(std::fs::File::from(probe).metadata()?.rdev())
}
