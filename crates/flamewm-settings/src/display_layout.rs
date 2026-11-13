//! Bounded Settings display arrangement. Geometry planning is pure. The WM owns the
//! native `RandR` effect: Settings sends typed placements through the control
//! keep/revert transaction, and persists the order only after the user keeps it.
use std::collections::BTreeSet;

use flamewm_api::display::{DisplaySnapshot, OutputPlacement, OutputSnapshot};
use flamewm_api::{OutputId, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPosition {
    pub id: OutputId,
    pub connector: String,
    pub after: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrangement {
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

/// Move an output one neighboring slot. The result is a contiguous horizontal strip
/// with no overlapping outputs. `Ok(None)` is a boundary/no-op.
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
            after,
        });
    }
    Ok(Some(Arrangement { outputs: planned }))
}

/// Typed `SetDisplayArrangement` payload for the WM transaction.
#[must_use]
pub fn placements(plan: &Arrangement) -> Vec<OutputPlacement> {
    plan.outputs
        .iter()
        .map(|output| OutputPlacement {
            output: output.id.clone(),
            x: output.after.x,
            y: output.after.y,
        })
        .collect()
}

/// Only the pre-existing two-output session persistence format can be written
/// here. Multi-output persistence requires the session owner's typed schema.
/// Call only after the WM reports a kept transaction.
pub fn persist_kept(plan: &Arrangement) -> Result<(), String> {
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
    use flamewm_api::{ModeId, Rect};

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
    fn swapped_unequal_resolutions_form_gap_free_strip() {
        let layout = plan(&snap(), &OutputId::new("HDMI-1"), Side::Left)
            .unwrap()
            .unwrap();
        assert_eq!(layout.outputs[0].id, OutputId::new("HDMI-1"));
        assert_eq!(layout.outputs[0].after, Rect::new(0, 0, 2560, 1440));
        assert_eq!(layout.outputs[1].after, Rect::new(2560, 0, 1920, 1080));
        assert_eq!(
            placements(&layout),
            vec![
                OutputPlacement {
                    output: OutputId::new("HDMI-1"),
                    x: 0,
                    y: 0,
                },
                OutputPlacement {
                    output: OutputId::new("DP-1"),
                    x: 2560,
                    y: 0,
                },
            ]
        );
    }
    #[test]
    fn four_outputs_adjacent_swap_normalizes_y_offsets() {
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
            .map(|o| (o.id.as_str(), o.after))
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
