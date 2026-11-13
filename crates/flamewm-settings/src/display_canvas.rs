//! Pure 2D arrangement model behind the Displays canvas. Positions are logical `RandR`
//! rectangles; nothing here touches the WM. The keyboard path nudges the selected output by
//! [`SNAP_STEP`], attaches it with [`display_layout::snap_drop`], and refuses overlap or any
//! move that disconnects the group. Enter commits through the keep/revert transaction.

use flamewm_api::display::{DisplaySnapshot, OutputPlacement};
use flamewm_api::{OutputId, Rect};

use crate::display_layout::{self, Arrangement, OutputPosition};

/// One keyboard step in logical pixels.
pub const SNAP_STEP: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasOutput {
    pub id: OutputId,
    pub connector: String,
    pub origin: Rect,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasLayout {
    outputs: Vec<CanvasOutput>,
    selected: usize,
    generation: u64,
}

impl CanvasLayout {
    /// Builds the canvas from the active outputs. Needs at least two connected outputs.
    pub fn from_snapshot(snapshot: &DisplaySnapshot, selected: &OutputId) -> Result<Self, String> {
        let outputs: Vec<CanvasOutput> = display_layout::active_outputs(snapshot)
            .into_iter()
            .map(|output| CanvasOutput {
                id: output.id.clone(),
                connector: output.connector.clone(),
                origin: output.geometry,
                rect: output.geometry,
            })
            .collect();
        if outputs.len() < 2 {
            return Err("at least two active monitors are required".into());
        }
        let selected = outputs
            .iter()
            .position(|output| &output.id == selected)
            .ok_or_else(|| "selected monitor is not an active RandR output".to_owned())?;
        let layout = Self {
            outputs,
            selected,
            generation: snapshot.generation,
        };
        if !layout.connected() {
            return Err("the current layout is not one connected group".into());
        }
        Ok(layout)
    }

    /// WM snapshot generation the positions were read at; a commit must still match it.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn selected_id(&self) -> &OutputId {
        &self.outputs[self.selected].id
    }

    /// Tab: next output in snapshot order, wrapping.
    pub fn select_next(&mut self) {
        self.selected = (self.selected + 1) % self.outputs.len();
    }

    /// Moves the selected output one [`SNAP_STEP`] toward `dir`, attached to the nearest
    /// legal edge. Errors leave the layout unchanged.
    pub fn nudge(&mut self, dir: Dir) -> Result<(), String> {
        let index = self.selected;
        let current = self.outputs[index].rect;
        let (dx, dy) = match dir {
            Dir::Left => (-SNAP_STEP, 0),
            Dir::Right => (SNAP_STEP, 0),
            Dir::Up => (0, -SNAP_STEP),
            Dir::Down => (0, SNAP_STEP),
        };
        let wanted = Rect::new(
            current.x + dx,
            current.y + dy,
            current.width,
            current.height,
        );
        let others: Vec<Rect> = self
            .outputs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, output)| output.rect)
            .collect();
        if let Some(blocker) = self
            .outputs
            .iter()
            .enumerate()
            .find(|(i, output)| *i != index && overlaps(output.rect, wanted))
            .map(|(_, output)| output.connector.clone())
        {
            return Err(format!("would overlap {blocker}"));
        }
        let placed = display_layout::snap_drop(
            &others,
            (current.width, current.height),
            (wanted.x, wanted.y),
        )
        .unwrap_or(wanted);
        if placed == current {
            return Err("no legal position in that direction".into());
        }
        let previous = self.outputs[index].rect;
        self.outputs[index].rect = placed;
        if !self.connected() {
            self.outputs[index].rect = previous;
            return Err("that move would disconnect the monitors".into());
        }
        Ok(())
    }

    /// Whether any output differs from the position the WM reported.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.outputs
            .iter()
            .any(|output| output.rect != output.origin)
    }

    /// Placements with the top-left of the group normalized to (0, 0), in snapshot order.
    #[must_use]
    pub fn placements(&self) -> Vec<OutputPlacement> {
        let (min_x, min_y) = self.origin_offset();
        self.outputs
            .iter()
            .map(|output| OutputPlacement {
                output: output.id.clone(),
                x: output.rect.x - min_x,
                y: output.rect.y - min_y,
            })
            .collect()
    }

    /// Typed arrangement for [`crate::SettingsApp`]'s keep path, normalized like `placements`.
    #[must_use]
    pub fn arrangement(&self) -> Arrangement {
        let (min_x, min_y) = self.origin_offset();
        Arrangement {
            outputs: self
                .outputs
                .iter()
                .map(|output| OutputPosition {
                    id: output.id.clone(),
                    connector: output.connector.clone(),
                    after: Rect::new(
                        output.rect.x - min_x,
                        output.rect.y - min_y,
                        output.rect.width,
                        output.rect.height,
                    ),
                })
                .collect(),
        }
    }

    /// Live status line for the canvas, naming the selected output and its position.
    #[must_use]
    pub fn status(&self) -> String {
        let output = &self.outputs[self.selected];
        format!(
            "{} at {}, {}{}",
            output.connector,
            output.rect.x,
            output.rect.y,
            if self.is_dirty() {
                ". Enter keeps this layout, Escape discards it"
            } else {
                ""
            }
        )
    }

    fn origin_offset(&self) -> (i32, i32) {
        (
            self.outputs.iter().map(|o| o.rect.x).min().unwrap_or(0),
            self.outputs.iter().map(|o| o.rect.y).min().unwrap_or(0),
        )
    }

    /// Every output reachable from the first through edge-touching neighbours.
    fn connected(&self) -> bool {
        let mut seen = vec![false; self.outputs.len()];
        let mut stack = vec![0usize];
        seen[0] = true;
        while let Some(current) = stack.pop() {
            for (next, flag) in seen.iter_mut().enumerate() {
                if !*flag && touches(self.outputs[current].rect, self.outputs[next].rect) {
                    *flag = true;
                    stack.push(next);
                }
            }
        }
        seen.iter().all(|reached| *reached)
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Edge-adjacent with a shared span of at least one pixel; overlap does not count.
fn touches(a: Rect, b: Rect) -> bool {
    let x_touch = a.x + a.width == b.x || b.x + b.width == a.x;
    let y_touch = a.y + a.height == b.y || b.y + b.height == a.y;
    let y_span = a.y < b.y + b.height && b.y < a.y + a.height;
    let x_span = a.x < b.x + b.width && b.x < a.x + a.width;
    (x_touch && y_span) || (y_touch && x_span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::display::{DisplayMode, OutputSnapshot};
    use flamewm_api::{ModeId, Size};

    fn output(id: &str, geometry: Rect) -> OutputSnapshot {
        OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: format!("edid-{id}"),
            connected: true,
            primary: false,
            geometry,
            current_mode: ModeId(1),
            modes: vec![DisplayMode {
                id: ModeId(1),
                resolution: Size::new(geometry.width, geometry.height),
                refresh_millihz: 60_000,
                preferred: true,
            }],
            shell_scale_percent: 100,
            rotation: flamewm_api::display::Rotation::Normal,
        }
    }

    fn snapshot(outputs: Vec<OutputSnapshot>) -> DisplaySnapshot {
        DisplaySnapshot {
            generation: 1,
            outputs,
            pending: None,
        }
    }

    fn pair() -> DisplaySnapshot {
        snapshot(vec![
            output("DP-1", Rect::new(0, 0, 1920, 1080)),
            output("HDMI-1", Rect::new(1920, 0, 1280, 720)),
        ])
    }

    #[test]
    fn fresh_canvas_is_clean_and_rejects_single_output() {
        let layout = CanvasLayout::from_snapshot(&pair(), &OutputId::new("HDMI-1")).expect("two");
        assert!(!layout.is_dirty());
        assert_eq!(layout.selected_id(), &OutputId::new("HDMI-1"));
        let single = snapshot(vec![output("DP-1", Rect::new(0, 0, 1920, 1080))]);
        assert!(CanvasLayout::from_snapshot(&single, &OutputId::new("DP-1")).is_err());
    }

    #[test]
    fn tab_cycles_selection_and_wraps() {
        let mut layout = CanvasLayout::from_snapshot(&pair(), &OutputId::new("DP-1")).expect("two");
        layout.select_next();
        assert_eq!(layout.selected_id(), &OutputId::new("HDMI-1"));
        layout.select_next();
        assert_eq!(layout.selected_id(), &OutputId::new("DP-1"));
    }

    #[test]
    fn nudge_slides_along_the_edge_and_stays_attached() {
        let mut layout =
            CanvasLayout::from_snapshot(&pair(), &OutputId::new("HDMI-1")).expect("two");
        layout.nudge(Dir::Down).expect("slide down along edge");
        let moved = layout.outputs[1].rect;
        assert_eq!(moved.x, 1920);
        assert!(moved.y > 0, "moved down by one step");
        assert!(touches(layout.outputs[0].rect, moved));
        assert!(layout.is_dirty());
    }

    #[test]
    fn nudge_into_overlap_or_disconnect_leaves_layout_unchanged() {
        let mut layout =
            CanvasLayout::from_snapshot(&pair(), &OutputId::new("HDMI-1")).expect("two");
        let before = layout.clone();
        assert!(layout.nudge(Dir::Left).is_err(), "overlap with DP-1");
        assert_eq!(layout, before);
        // A gap to the east is snapped back onto DP-1's edge, so there is no legal move.
        assert!(
            layout.nudge(Dir::Right).is_err(),
            "gap snaps back to the edge"
        );
        assert_eq!(layout, before);
    }

    #[test]
    fn placements_normalize_group_origin_to_zero() {
        let mut layout = CanvasLayout::from_snapshot(&pair(), &OutputId::new("DP-1")).expect("two");
        layout.outputs[0].rect = Rect::new(-40, 100, 1920, 1080);
        layout.outputs[1].rect = Rect::new(1880, 100, 1280, 720);
        let placements = layout.placements();
        assert_eq!((placements[0].x, placements[0].y), (0, 0));
        assert_eq!((placements[1].x, placements[1].y), (1920, 0));
        assert_eq!(
            layout.arrangement().outputs[1].after,
            Rect::new(1920, 0, 1280, 720)
        );
    }

    #[test]
    fn status_names_output_and_offers_keep_only_when_dirty() {
        let mut layout =
            CanvasLayout::from_snapshot(&pair(), &OutputId::new("HDMI-1")).expect("two");
        assert_eq!(layout.status(), "HDMI-1 at 1920, 0");
        layout.nudge(Dir::Down).expect("slide");
        assert!(layout.status().contains("Enter keeps"));
    }
}
