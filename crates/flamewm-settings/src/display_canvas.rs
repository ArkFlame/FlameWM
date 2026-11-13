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

/// Markup nodes available for the canvas; outputs beyond this are not drawn.
pub const CANVAS_NODES: usize = 8;

/// Pointer drag on one canvas node. `start` is the node position at press time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasDrag {
    pub index: usize,
    pub pointer: (f32, f32),
    pub start: (i32, i32),
    pub moved: bool,
}

/// Canvas size in CSS pixels, and the margin kept around the scaled group.
pub const CANVAS_WIDTH: f32 = 520.0;
pub const CANVAS_HEIGHT: f32 = 220.0;
const MARGIN: f32 = 12.0;

/// Outcome of dropping an output at a logical position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preview {
    pub rect: Rect,
    pub legal: bool,
    /// Index of the neighbour the drop attached to, when `snap_drop` found an edge.
    pub snapped_to: Option<usize>,
}

/// Fixed scale and origin that fit the reported group into the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CanvasView {
    scale: f32,
    left: i32,
    top: i32,
}

impl CanvasView {
    fn fit(outputs: &[CanvasOutput]) -> Self {
        let left = outputs.iter().map(|o| o.origin.x).min().unwrap_or(0);
        let top = outputs.iter().map(|o| o.origin.y).min().unwrap_or(0);
        let right = outputs
            .iter()
            .map(|o| o.origin.x + o.origin.width)
            .max()
            .unwrap_or(1);
        let bottom = outputs
            .iter()
            .map(|o| o.origin.y + o.origin.height)
            .max()
            .unwrap_or(1);
        let width = (right - left).max(1) as f32;
        let height = (bottom - top).max(1) as f32;
        let scale =
            ((CANVAS_WIDTH - 2.0 * MARGIN) / width).min((CANVAS_HEIGHT - 2.0 * MARGIN) / height);
        Self { scale, left, top }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanvasLayout {
    outputs: Vec<CanvasOutput>,
    selected: usize,
    generation: u64,
    view: CanvasView,
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
        let view = CanvasView::fit(&outputs);
        let layout = Self {
            outputs,
            selected,
            generation: snapshot.generation,
            view,
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
    pub fn outputs(&self) -> &[CanvasOutput] {
        &self.outputs
    }

    #[must_use]
    pub const fn selected_index(&self) -> usize {
        self.selected
    }

    /// Selects the output at `index`; out-of-range indexes are ignored.
    pub fn select_index(&mut self, index: usize) {
        if index < self.outputs.len() {
            self.selected = index;
        }
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

    /// Node rectangles inside the bounded canvas, in CSS pixels: `(left, top, width, height)`.
    #[must_use]
    pub fn node_px(&self, index: usize) -> (f32, f32, f32, f32) {
        let rect = self.outputs[index].rect;
        let view = self.view;
        (
            MARGIN + (rect.x - view.left) as f32 * view.scale,
            MARGIN + (rect.y - view.top) as f32 * view.scale,
            rect.width as f32 * view.scale,
            rect.height as f32 * view.scale,
        )
    }

    /// Converts a pointer delta in CSS pixels to logical pixels at the canvas scale.
    #[must_use]
    pub fn px_to_logical(&self, dx: f32, dy: f32) -> (i32, i32) {
        (
            (dx / self.view.scale).round() as i32,
            (dy / self.view.scale).round() as i32,
        )
    }

    /// Where `index` would land if dropped at `wanted`: attached through `snap_drop` when an
    /// edge is near, otherwise free. `legal` is false on overlap or a disconnected group.
    #[must_use]
    pub fn preview(&self, index: usize, wanted: (i32, i32)) -> Preview {
        let size = (
            self.outputs[index].rect.width,
            self.outputs[index].rect.height,
        );
        let others: Vec<(usize, Rect)> = self
            .outputs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(i, output)| (i, output.rect))
            .collect();
        let rects: Vec<Rect> = others.iter().map(|(_, rect)| *rect).collect();
        let snapped = display_layout::snap_drop(&rects, size, wanted);
        let rect = snapped.unwrap_or_else(|| Rect::new(wanted.0, wanted.1, size.0, size.1));
        let snapped_to = snapped.and_then(|placed| {
            others
                .iter()
                .find(|(_, other)| touches(placed, *other))
                .map(|(i, _)| *i)
        });
        let mut trial = self.clone();
        trial.outputs[index].rect = rect;
        let legal = !others.iter().any(|(_, other)| overlaps(*other, rect)) && trial.connected();
        Preview {
            rect,
            legal,
            snapped_to,
        }
    }

    /// Applies a drag step when legal; an illegal step leaves the last legal position.
    pub fn drag_to(&mut self, index: usize, wanted: (i32, i32)) -> Preview {
        let preview = self.preview(index, wanted);
        if preview.legal {
            self.outputs[index].rect = preview.rect;
        }
        preview
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

#[cfg(test)]
mod preview_tests {
    use super::*;
    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot, Rotation};
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
            rotation: Rotation::Normal,
        }
    }

    #[test]
    fn drag_onto_another_output_is_blocked_and_keeps_the_last_legal_position() {
        let snapshot = DisplaySnapshot {
            generation: 1,
            outputs: vec![
                output("DP-1", Rect::new(0, 0, 1920, 1080)),
                output("HDMI-1", Rect::new(1920, 0, 1280, 720)),
            ],
            pending: None,
        };
        let mut layout =
            CanvasLayout::from_snapshot(&snapshot, &OutputId::new("HDMI-1")).expect("two");
        let before = layout.outputs()[1].rect;
        // Dropping HDMI-1 on top of DP-1 overlaps it, so the preview is illegal.
        let blocked = layout.drag_to(1, (100, 100));
        assert!(!blocked.legal);
        assert_eq!(layout.outputs()[1].rect, before);
        assert!(!layout.is_dirty());
        // A legal edge drop applies and reports the neighbour it attached to.
        let attached = layout.drag_to(1, (1920, 200));
        assert!(attached.legal);
        assert_eq!(attached.snapped_to, Some(0));
        assert!(layout.is_dirty());
    }
}
