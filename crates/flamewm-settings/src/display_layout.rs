//! Bounded Settings display arrangement. Geometry planning is pure; native effects
//! occur only after explicit user release and fresh snapshot validation.
//!
//! The current control protocol has no atomic RandR topology mutation. Until it
//! does, this is a single argv-based xrandr invocation with mandatory readback,
//! not a second cached display authority. No shell interpolation or modeset flags.
use std::collections::BTreeSet;
use std::process::Command;

use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
use flamewm_api::{ModeId, OutputId, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPosition {
    pub id: OutputId,
    pub connector: String,
    pub before: Rect,
    pub after: Rect,
    pub mode: ModeId,
    pub primary: bool,
    pub scale: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrangement {
    pub generation: u64,
    pub outputs: Vec<OutputPosition>,
}

#[must_use]
pub fn active_outputs(snapshot: &DisplaySnapshot) -> Vec<&OutputSnapshot> {
    let mut outputs: Vec<_> = snapshot
        .outputs
        .iter()
        .filter(|o| o.connected && o.geometry.is_valid() && o.current_mode.0 != 0)
        .collect();
    outputs.sort_by_key(|o| (o.geometry.x, o.geometry.y, o.connector.clone()));
    outputs
}

/// Move an output one neighboring slot, retaining every active mode, rotation,
/// primary flag and per-output shell scale. The resulting contiguous horizontal
/// strip has no transient/overlapping CRTC positions to depend on.
/// `Ok(None)` is a boundary/no-op, and must never issue a RandR request.
pub fn plan(
    snapshot: &DisplaySnapshot,
    output: &OutputId,
    side: Side,
) -> Result<Option<Arrangement>, String> {
    if snapshot.pending.is_some() {
        return Err("finish the pending display mode change before rearranging".into());
    }
    let mut active = active_outputs(snapshot);
    if active.len() < 2 {
        return Err("at least two active monitors are required".into());
    }
    let mut connectors = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for candidate in &active {
        if candidate.connector.trim().is_empty()
            || candidate.connector.starts_with('-')
            || !connectors.insert(candidate.connector.clone())
            || !ids.insert(candidate.id.clone())
        {
            return Err("ambiguous RandR output connector or identity".into());
        }
    }
    let position = active
        .iter()
        .position(|candidate| candidate.id == *output)
        .ok_or_else(|| "selected monitor is not an active RandR output".to_owned())?;
    let next = match side {
        Side::Left => position.checked_sub(1),
        Side::Right => (position + 1 < active.len()).then_some(position + 1),
    };
    let Some(next) = next else {
        return Ok(None);
    };
    active.swap(position, next);

    let min_y = active
        .iter()
        .map(|candidate| candidate.geometry.y)
        .min()
        .unwrap_or(0);
    let mut x = 0i32;
    let mut planned = Vec::with_capacity(active.len());
    for candidate in active {
        let y = candidate
            .geometry
            .y
            .checked_sub(min_y)
            .ok_or_else(|| "display Y coordinate overflow".to_owned())?;
        let after = Rect::new(x, y, candidate.geometry.width, candidate.geometry.height);
        x = x
            .checked_add(candidate.geometry.width)
            .ok_or_else(|| "display framebuffer exceeds supported coordinates".to_owned())?;
        if y.checked_add(candidate.geometry.height).is_none() {
            return Err("display framebuffer geometry overflow".into());
        }
        planned.push(OutputPosition {
            id: candidate.id.clone(),
            connector: candidate.connector.clone(),
            before: candidate.geometry,
            after,
            mode: candidate.current_mode,
            primary: candidate.primary,
            scale: candidate.shell_scale_percent,
        });
    }
    Ok(Some(Arrangement {
        generation: snapshot.generation,
        outputs: planned,
    }))
}

/// One xrandr invocation updates every active output. --fb covers unequal-width
/// rotations before the output positions are applied; no mode/rate/scale flags.
pub fn placement_args(plan: &Arrangement, previous: bool) -> Result<Vec<String>, String> {
    if plan.outputs.is_empty() {
        return Err("empty monitor layout".into());
    }
    let mut args = Vec::with_capacity(plan.outputs.len() * 4 + 2);
    let mut max_right = 0i32;
    let mut max_bottom = 0i32;
    for output in &plan.outputs {
        let rect = if previous {
            output.before
        } else {
            output.after
        };
        if !rect.is_valid() || rect.x < 0 || rect.y < 0 {
            return Err(
                "negative or invalid framebuffer placement cannot be sent to xrandr".into(),
            );
        }
        max_right = max_right.max(
            rect.x
                .checked_add(rect.width)
                .ok_or_else(|| "display framebuffer X overflow".to_owned())?,
        );
        max_bottom = max_bottom.max(
            rect.y
                .checked_add(rect.height)
                .ok_or_else(|| "display framebuffer Y overflow".to_owned())?,
        );
    }
    args.extend(["--fb".to_owned(), format!("{max_right}x{max_bottom}")]);
    for output in &plan.outputs {
        let rect = if previous {
            output.before
        } else {
            output.after
        };
        args.extend([
            "--output".to_owned(),
            output.connector.clone(),
            "--pos".to_owned(),
            format!("{}x{}", rect.x, rect.y),
        ]);
    }
    Ok(args)
}

pub fn apply(plan: &Arrangement) -> Result<(), String> {
    let args = placement_args(plan, false)?;
    let result = Command::new("xrandr")
        .args(&args)
        .output()
        .map_err(|error| format!("xrandr could not apply monitor layout: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "RandR rejected monitor layout: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

/// Reject success based only on the subprocess exit code. A true commit needs
/// a new authoritative WM snapshot with identical modes/primary/scale/positions.
pub fn verify(plan: &Arrangement, current: &DisplaySnapshot) -> Result<(), String> {
    if current.pending.is_some() {
        return Err("a display mode confirmation became pending during arrangement".into());
    }
    let active = active_outputs(current);
    if active.len() != plan.outputs.len() {
        return Err("active output topology changed during arrangement".into());
    }
    for intended in &plan.outputs {
        let observed = active
            .iter()
            .find(|output| output.id == intended.id && output.connector == intended.connector)
            .ok_or_else(|| format!("{} disappeared during arrangement", intended.connector))?;
        if observed.geometry != intended.after
            || observed.current_mode != intended.mode
            || observed.primary != intended.primary
            || observed.shell_scale_percent != intended.scale
        {
            return Err(format!(
                "{} did not reach its committed geometry and mode",
                intended.connector
            ));
        }
    }
    Ok(())
}

/// Rollback is permissible only if the same physical output identities,
/// primary state and display modes still exist. A hotplug or concurrent mode
/// change must not be overridden with coordinates from an obsolete snapshot.
pub fn rollback_if_safe(plan: &Arrangement, observed: &DisplaySnapshot) -> Result<(), String> {
    if observed.pending.is_some() || active_outputs(observed).len() != plan.outputs.len() {
        return Err("cannot roll back changed monitor topology".into());
    }
    for old in &plan.outputs {
        if !active_outputs(observed).iter().any(|output| {
            output.id == old.id
                && output.connector == old.connector
                && output.current_mode == old.mode
                && output.primary == old.primary
        }) {
            return Err("cannot roll back a changed output identity or mode".into());
        }
    }
    let args = placement_args(plan, true)?;
    let result = Command::new("xrandr")
        .args(args)
        .output()
        .map_err(|error| format!("could not start RandR rollback: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "RandR rollback rejected: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

/// Only the pre-existing two-output session persistence format can be written
/// here. Multi-output persistence requires the session owner's typed schema.
pub fn persist_verified(plan: &Arrangement) -> Result<(), String> {
    if plan.outputs.len() != 2 {
        return Err(
            "multi-output layout persistence requires the session owner's updated schema".into(),
        );
    }
    let left = &plan.outputs[0].connector;
    let right = &plan.outputs[1].connector;
    flamewm_session_core::display_order::save_order(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::Rect;

    fn output(id: &str, x: i32, y: i32, width: i32, height: i32) -> OutputSnapshot {
        OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: id.to_owned(),
            connected: true,
            primary: id == "HDMI-1",
            geometry: Rect::new(x, y, width, height),
            current_mode: ModeId(1),
            modes: vec![],
            shell_scale_percent: 125,
        }
    }
    fn snap() -> DisplaySnapshot {
        DisplaySnapshot {
            generation: 2,
            pending: None,
            outputs: vec![
                output("HDMI-1", 1920, 0, 2560, 1440),
                output("DP-1", 0, 0, 1920, 1080),
            ],
        }
    }
    #[test]
    fn swapped_unequal_resolutions_have_correct_nonoverlapping_framebuffer() {
        let snapshot = snap();
        let layout = plan(&snapshot, &OutputId::new("HDMI-1"), Side::Left)
            .unwrap()
            .unwrap();
        assert_eq!(layout.outputs[0].connector, "HDMI-1");
        assert_eq!(layout.outputs[0].after, Rect::new(0, 0, 2560, 1440));
        assert_eq!(layout.outputs[1].after, Rect::new(2560, 0, 1920, 1080));
        assert_eq!(
            placement_args(&layout, false).unwrap().join(" "),
            "--fb 4480x1440 --output HDMI-1 --pos 0x0 --output DP-1 --pos 2560x0"
        );
        let mut readback = snapshot;
        for new in &layout.outputs {
            readback
                .outputs
                .iter_mut()
                .find(|o| o.id == new.id)
                .unwrap()
                .geometry = new.after;
        }
        assert!(verify(&layout, &readback).is_ok());
        readback.outputs[1].geometry.width = 2560;
        assert!(verify(&layout, &readback).is_err());
    }
    #[test]
    fn four_outputs_adjacent_swap_preserves_modes_and_y_offsets() {
        let mut snapshot = snap();
        snapshot
            .outputs
            .push(output("DP-2", 4480, -100, 1200, 1920));
        snapshot.outputs.push(output("eDP-1", 5680, 0, 1280, 800));
        let layout = plan(&snapshot, &OutputId::new("DP-2"), Side::Left)
            .unwrap()
            .unwrap();
        let positions: Vec<_> = layout
            .outputs
            .iter()
            .map(|o| (o.connector.as_str(), o.after))
            .collect();
        assert_eq!(
            positions,
            vec![
                ("DP-1", Rect::new(0, 100, 1920, 1080)),
                ("DP-2", Rect::new(1920, 0, 1200, 1920)),
                ("HDMI-1", Rect::new(3120, 100, 2560, 1440)),
                ("eDP-1", Rect::new(5680, 100, 1280, 800)),
            ]
        );
        assert_eq!(layout.outputs[2].primary, true);
        assert_eq!(layout.outputs[2].scale, 125);
    }
    #[test]
    fn no_effect_at_boundaries_and_untrusted_outputs_rejected() {
        let snapshot = snap();
        assert!(
            plan(&snapshot, &OutputId::new("DP-1"), Side::Left)
                .unwrap()
                .is_none()
        );
        assert!(plan(&snapshot, &OutputId::new("BAD"), Side::Right).is_err());
        let mut snapshot = snapshot;
        snapshot.pending = Some(flamewm_api::display::PendingModeChange {
            transaction: flamewm_api::TransactionId(3),
            output: OutputId::new("DP-1"),
            mode: ModeId(1),
            deadline_ms: 1,
        });
        assert!(plan(&snapshot, &OutputId::new("DP-1"), Side::Right).is_err());
    }
    #[test]
    fn disconnected_and_duplicate_connectors_cannot_be_applied() {
        let mut snapshot = snap();
        snapshot.outputs[1].connected = false;
        assert!(plan(&snapshot, &OutputId::new("DP-1"), Side::Right).is_err());
        snapshot = snap();
        snapshot.outputs[1].connector = "HDMI-1".into();
        assert!(plan(&snapshot, &OutputId::new("DP-1"), Side::Right).is_err());
    }
}
