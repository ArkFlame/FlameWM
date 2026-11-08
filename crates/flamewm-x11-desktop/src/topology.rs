//! RandR topology transactions: primary output, panel rotation and output placement.
//!
//! The baseline records every active CRTC and the primary output, so keep/revert can restore the
//! whole layout rather than one CRTC like a mode change.

// X11 protocol names come from the crate root, which owns the x11rb connection and imports.
use super::*;
use flamewm_api::display::{OutputPlacement, Rotation, TopologyChange};

#[derive(Debug, Clone)]
struct CrtcState {
    crtc: randr::Crtc,
    x: i16,
    y: i16,
    mode: randr::Mode,
    rotation: randr::Rotation,
    outputs: Vec<randr::Output>,
}

/// Layout state captured before a topology change, restored on revert.
#[derive(Debug, Clone)]
pub struct Baseline {
    primary: randr::Output,
    crtcs: Vec<CrtcState>,
}

/// A topology change applied but not yet kept.
#[derive(Debug, Clone)]
pub struct PendingTopology {
    pub transaction: TransactionId,
    pub baseline: Baseline,
}

/// Applies `change` and returns the baseline needed to revert it. A failed apply restores the
/// baseline before returning its error.
pub fn apply(conn: &RustConnection, root: Window, change: &TopologyChange) -> FlameResult<Baseline> {
    let baseline = capture(conn, root)?;
    if let Err(error) = apply_change(conn, root, change) {
        // Best effort: the original error is the one the caller needs.
        let _ = restore(conn, root, &baseline);
        return Err(error);
    }
    conn.flush().map_err(FlameError::io)?;
    Ok(baseline)
}

/// Restores the captured CRTC layout and primary output.
pub fn restore(conn: &RustConnection, root: Window, baseline: &Baseline) -> FlameResult<()> {
    let resources = resources(conn, root)?;
    for state in &baseline.crtcs {
        set_crtc(conn, state.crtc, &resources, state)?;
    }
    set_primary(conn, root, baseline.primary)?;
    conn.flush().map_err(FlameError::io)
}

fn capture(conn: &RustConnection, root: Window) -> FlameResult<Baseline> {
    let resources = resources(conn, root)?;
    let primary = conn
        .randr_get_output_primary(root)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?
        .output;
    let mut crtcs = Vec::new();
    for &crtc in &resources.crtcs {
        let info = crtc_info(conn, crtc, &resources)?;
        if info.mode == 0 {
            continue;
        }
        crtcs.push(CrtcState {
            crtc,
            x: info.x,
            y: info.y,
            mode: info.mode,
            rotation: info.rotation,
            outputs: info.outputs,
        });
    }
    Ok(Baseline { primary, crtcs })
}

fn apply_change(conn: &RustConnection, root: Window, change: &TopologyChange) -> FlameResult<()> {
    let resources = resources(conn, root)?;
    match change {
        TopologyChange::SetPrimary(name) => {
            let (output, _) = find_output(conn, &resources, name)?;
            set_primary(conn, root, output)
        }
        TopologyChange::SetOrientation { output, rotation } => {
            let crtc = active_crtc(conn, &resources, output)?;
            let info = crtc_info(conn, crtc, &resources)?;
            let bit = rotation_bit(*rotation);
            if u16::from(info.rotations) & u16::from(bit) == 0 {
                return Err(FlameError::invalid(
                    "RandR output does not support this rotation",
                ));
            }
            let state = CrtcState {
                crtc,
                x: info.x,
                y: info.y,
                mode: info.mode,
                rotation: bit,
                outputs: info.outputs,
            };
            set_crtc(conn, crtc, &resources, &state)
        }
        TopologyChange::SetArrangement(placements) => {
            arrange(conn, root, &resources, placements)
        }
    }
}

fn arrange(
    conn: &RustConnection,
    root: Window,
    resources: &randr::GetScreenResourcesCurrentReply,
    placements: &[OutputPlacement],
) -> FlameResult<()> {
    let screen = conn
        .get_geometry(root)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?;
    // Validate every placement before the first CRTC change, so a rejected plan changes nothing.
    // shortcut: X screen resize (randr set_screen_size) is not implemented; placements beyond the
    // current screen are refused. Revisit when multi-output runtime evidence exists.
    let mut plan = Vec::with_capacity(placements.len());
    for placement in placements {
        let crtc = active_crtc(conn, resources, &placement.output)?;
        let info = crtc_info(conn, crtc, resources)?;
        if !within_screen(
            placement.x,
            placement.y,
            info.width,
            info.height,
            (screen.width, screen.height),
        ) {
            return Err(FlameError::invalid(
                "arrangement exceeds the X screen; screen resize is not implemented",
            ));
        }
        let x = i16::try_from(placement.x).map_err(|_| FlameError::invalid("placement x is out of range"))?;
        let y = i16::try_from(placement.y).map_err(|_| FlameError::invalid("placement y is out of range"))?;
        plan.push(CrtcState {
            crtc,
            x,
            y,
            mode: info.mode,
            rotation: info.rotation,
            outputs: info.outputs,
        });
    }
    for state in &plan {
        set_crtc(conn, state.crtc, resources, state)?;
    }
    Ok(())
}

/// Maps the typed quarter-turn to the RandR rotation bit (clockwise from native orientation).
#[must_use]
pub fn rotation_bit(rotation: Rotation) -> randr::Rotation {
    match rotation {
        Rotation::Normal => randr::Rotation::ROTATE0,
        Rotation::Right => randr::Rotation::ROTATE90,
        Rotation::Inverted => randr::Rotation::ROTATE180,
        Rotation::Left => randr::Rotation::ROTATE270,
    }
}

/// True when a `width` x `height` rectangle at (`x`, `y`) lies inside a `screen` sized X screen.
#[must_use]
pub fn within_screen(x: i32, y: i32, width: u16, height: u16, screen: (u16, u16)) -> bool {
    x >= 0
        && y >= 0
        && x + i32::from(width) <= i32::from(screen.0)
        && y + i32::from(height) <= i32::from(screen.1)
}

fn resources(
    conn: &RustConnection,
    root: Window,
) -> FlameResult<randr::GetScreenResourcesCurrentReply> {
    conn.randr_get_screen_resources_current(root)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)
}

fn crtc_info(
    conn: &RustConnection,
    crtc: randr::Crtc,
    resources: &randr::GetScreenResourcesCurrentReply,
) -> FlameResult<randr::GetCrtcInfoReply> {
    conn.randr_get_crtc_info(crtc, resources.config_timestamp)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)
}

fn find_output(
    conn: &RustConnection,
    resources: &randr::GetScreenResourcesCurrentReply,
    name: &OutputId,
) -> FlameResult<(randr::Output, randr::GetOutputInfoReply)> {
    for &output in &resources.outputs {
        let info = conn
            .randr_get_output_info(output, resources.config_timestamp)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        if String::from_utf8_lossy(&info.name) == name.as_str() {
            return Ok((output, info));
        }
    }
    Err(FlameError::not_found("RandR output not found"))
}

fn active_crtc(
    conn: &RustConnection,
    resources: &randr::GetScreenResourcesCurrentReply,
    name: &OutputId,
) -> FlameResult<randr::Crtc> {
    let (_, info) = find_output(conn, resources, name)?;
    if u8::from(info.connection) != u8::from(randr::Connection::CONNECTED) || info.crtc == 0 {
        return Err(FlameError::invalid("RandR output is not active"));
    }
    Ok(info.crtc)
}

fn set_crtc(
    conn: &RustConnection,
    crtc: randr::Crtc,
    resources: &randr::GetScreenResourcesCurrentReply,
    state: &CrtcState,
) -> FlameResult<()> {
    let reply = conn
        .randr_set_crtc_config(
            crtc,
            CURRENT_TIME,
            resources.config_timestamp,
            state.x,
            state.y,
            state.mode,
            state.rotation,
            &state.outputs,
        )
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?;
    if u8::from(reply.status) != u8::from(randr::SetConfig::SUCCESS) {
        return Err(FlameError::new(
            ErrorCode::EngineRejected,
            "RandR rejected topology change",
        ));
    }
    Ok(())
}

fn set_primary(conn: &RustConnection, root: Window, output: randr::Output) -> FlameResult<()> {
    conn.randr_set_output_primary(root, output)
        .map_err(FlameError::io)?
        .check()
        .map_err(FlameError::io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_map_clockwise_from_native() {
        assert_eq!(rotation_bit(Rotation::Normal), randr::Rotation::ROTATE0);
        assert_eq!(rotation_bit(Rotation::Right), randr::Rotation::ROTATE90);
        assert_eq!(rotation_bit(Rotation::Inverted), randr::Rotation::ROTATE180);
        assert_eq!(rotation_bit(Rotation::Left), randr::Rotation::ROTATE270);
    }

    #[test]
    fn placement_must_fit_the_screen() {
        assert!(within_screen(0, 0, 1920, 1080, (3840, 1080)));
        assert!(within_screen(1920, 0, 1920, 1080, (3840, 1080)));
        assert!(!within_screen(1921, 0, 1920, 1080, (3840, 1080)));
        assert!(!within_screen(-1, 0, 1, 1, (10, 10)));
        assert!(!within_screen(0, 0, 1080, 1920, (1920, 1080)));
    }
}
