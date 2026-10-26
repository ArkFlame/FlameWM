//! Output geometry and the projected-cell cache: the root surface, per-output grids,
//! pointer-to-output mapping and panel subtraction. Moved verbatim from `main.rs`.

use super::*;

/// Root geometry comes from the actual X root through the X11 runtime owner.
/// The root-sized surface paints wallpaper under panels; per-output grids subtract panel geometry.
/// The desktop surface spans the whole root at origin 0,0 so the wallpaper paints under panels
/// and struts. Icon layout and selection stay in the work area (`GridConfig::work_area`).
pub(super) fn desktop_surface_rect(root_width: u32, root_height: u32) -> Rect {
    Rect::new(0, 0, root_width.max(1) as i32, root_height.max(1) as i32)
}

pub(super) fn resolve_output_layout(
    surface: Rect,
    entry_scale_percent: u16,
) -> (
    Vec<(OutputId, Rect)>,
    BTreeMap<OutputId, GridConfig>,
    OutputId,
) {
    let outputs = control_client()
        .and_then(|client| {
            match client.call_with_timeout(
                &ControlRequest::GetDisplays,
                std::time::Duration::from_secs(2),
            ) {
                Ok(ControlResponse::Displays(snapshot)) => Ok(snapshot
                    .outputs
                    .into_iter()
                    .filter(|output| output.connected)
                    .collect::<Vec<_>>()),
                Ok(response) => Err(format!("GetDisplays returned {response:?}")),
                Err(error) => Err(error.message),
            }
        })
        .unwrap_or_else(|error| {
            eprintln!("desktop: degraded monitor geometry: {error}; using full root geometry");
            Vec::new()
        });
    let scales = outputs
        .iter()
        .map(|output| (output.id.clone(), output.shell_scale_percent))
        .collect::<BTreeMap<_, _>>();
    let raw = outputs
        .iter()
        .map(|output| (output.id.clone(), output.geometry, output.primary))
        .collect::<Vec<_>>();
    let (mut geometries, origin_x, origin_y) = normalize_outputs(&raw, surface);
    let default_output = outputs
        .iter()
        .find(|output| output.primary && geometries.iter().any(|(id, _)| id == &output.id))
        .map(|output| output.id.clone())
        .or_else(|| geometries.first().map(|(id, _)| id.clone()))
        .unwrap_or_else(|| OutputId::new("default"));
    if geometries.is_empty() {
        geometries.push((default_output.clone(), surface));
    }
    let panels = match panel_geometries() {
        Ok(panels) => panels,
        Err(error) => {
            eprintln!("desktop: degraded work areas: {error}; using monitor bounds");
            Vec::new()
        }
    };
    let panels = panels
        .into_iter()
        .map(|(id, rect)| {
            (
                id,
                Rect::new(
                    rect.x.saturating_sub(origin_x),
                    rect.y.saturating_sub(origin_y),
                    rect.width,
                    rect.height,
                ),
            )
        })
        .collect::<Vec<_>>();
    let grids = grids_for_outputs(&geometries, &panels, &scales, entry_scale_percent);
    (geometries, grids, default_output)
}

pub(super) fn grids_for_outputs(
    outputs: &[(OutputId, Rect)],
    panels: &[(OutputId, Rect)],
    scales: &BTreeMap<OutputId, u16>,
    entry_scale_percent: u16,
) -> BTreeMap<OutputId, GridConfig> {
    outputs
        .iter()
        .map(|(output, geometry)| {
            let area = panels
                .iter()
                .filter(|(panel_output, _)| panel_output == output)
                .fold(*geometry, |area, (_, panel)| {
                    subtract_panel_strip(area, *panel)
                });
            (
                output.clone(),
                GridConfig::compute_with_entry_scale(
                    area,
                    scales.get(output).copied().unwrap_or(100),
                    entry_scale_percent,
                ),
            )
        })
        .collect()
}

pub(super) fn normalize_outputs(
    raw: &[(OutputId, Rect, bool)],
    surface: Rect,
) -> (Vec<(OutputId, Rect)>, i32, i32) {
    let origin_x = raw.iter().map(|(_, rect, _)| rect.x).min().unwrap_or(0);
    let origin_y = raw.iter().map(|(_, rect, _)| rect.y).min().unwrap_or(0);
    let outputs = raw
        .iter()
        .filter_map(|(id, rect, _)| {
            Rect::new(
                rect.x.saturating_sub(origin_x),
                rect.y.saturating_sub(origin_y),
                rect.width,
                rect.height,
            )
            .intersection(surface)
            .map(|rect| (id.clone(), rect))
        })
        .collect();
    (outputs, origin_x, origin_y)
}

pub(super) fn refresh_output_layout(
    surface: Rect,
    entry_scale_percent: u16,
) -> (
    Vec<(OutputId, Rect)>,
    BTreeMap<OutputId, GridConfig>,
    OutputId,
) {
    resolve_output_layout(surface, entry_scale_percent)
}

pub(super) fn grid_for_output(state: &DesktopState, output: &OutputId) -> GridConfig {
    state
        .output_grids
        .get(output)
        .copied()
        .unwrap_or(state.grid)
}

/// The output an item lays out on: its own output while it is still connected, else the default.
pub(super) fn live_output(state: &DesktopState, output: &OutputId) -> OutputId {
    if state.output_grids.contains_key(output) {
        output.clone()
    } else {
        state.default_output.clone()
    }
}

fn output_at(state: &DesktopState, point: Point) -> OutputId {
    output_at_in(&state.output_geometries, &state.default_output, point)
}

pub(super) fn output_at_in(
    outputs: &[(OutputId, Rect)],
    fallback: &OutputId,
    point: Point,
) -> OutputId {
    outputs
        .iter()
        .find(|(_, rect)| rect.contains(point))
        .or_else(|| {
            outputs.iter().min_by_key(|(_, rect)| {
                let x = point.x.clamp(rect.x, rect.right().saturating_sub(1));
                let y = point.y.clamp(rect.y, rect.bottom().saturating_sub(1));
                (i64::from(point.x) - i64::from(x)).pow(2)
                    + (i64::from(point.y) - i64::from(y)).pow(2)
            })
        })
        .map_or_else(|| fallback.clone(), |(id, _)| id.clone())
}

pub(super) fn pointer_on_output(state: &DesktopState, x: f32, y: f32) -> (OutputId, Point) {
    let raw = Point::new(x.round() as i32, y.round() as i32);
    let output = output_at(state, raw);
    let point = pointer_point_for_grid(raw, grid_for_output(state, &output));
    (output, point)
}

pub(super) fn pointer_point_for_grid(point: Point, grid: GridConfig) -> Point {
    let bounds = grid.work_area;
    Point::new(
        point.x.clamp(bounds.x, bounds.right().saturating_sub(1)),
        point.y.clamp(bounds.y, bounds.bottom().saturating_sub(1)),
    )
}

#[cfg(test)]
thread_local! {
    pub(super) static PROJECTED_CELLS_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `projected_cells` output plus every input it reads: the model item outputs and cells,
/// `output_grids`, `default_output` and the fallback `grid`. Reuse is gated by comparing
/// these inputs against live state, so no mutation path needs to invalidate it.
pub(super) struct ProjectedCellsCache {
    pub(super) grid: GridConfig,
    pub(super) output_grids: BTreeMap<OutputId, GridConfig>,
    pub(super) default_output: OutputId,
    pub(super) items: Vec<(OutputId, Cell)>,
    pub(super) cells: Vec<(OutputId, Cell)>,
}

impl ProjectedCellsCache {
    pub(super) fn capture(state: &DesktopState) -> Self {
        Self {
            grid: state.grid,
            output_grids: state.output_grids.clone(),
            default_output: state.default_output.clone(),
            items: state
                .model
                .items()
                .map(|item| (item.output.clone(), item.cell))
                .collect(),
            cells: projected_cells(state),
        }
    }

    pub(super) fn matches(&self, state: &DesktopState) -> bool {
        if self.grid != state.grid
            || self.default_output != state.default_output
            || self.output_grids != state.output_grids
        {
            return false;
        }
        let mut live = state.model.items();
        self.items.iter().all(|(output, cell)| {
            live.next()
                .is_some_and(|item| *output == item.output && *cell == item.cell)
        }) && live.next().is_none()
    }
}

/// Projected `(output, cell)` for every model item, aligned by index with
/// `state.model.items()` (so `cells[i]` belongs to the i-th item).
pub(super) fn projected_cells(state: &DesktopState) -> Vec<(OutputId, Cell)> {
    #[cfg(test)]
    PROJECTED_CELLS_CALLS.with(|calls| calls.set(calls.get() + 1));
    let items: Vec<_> = state.model.items().collect();
    let mut result: Vec<Option<(OutputId, Cell)>> = vec![None; items.len()];
    let mut occupied = BTreeMap::<OutputId, BTreeSet<Cell>>::new();
    for (slot, item) in items
        .iter()
        .enumerate()
        .filter(|(_, item)| state.output_grids.contains_key(&item.output))
    {
        let grid = grid_for_output(state, &item.output);
        let cell = grid.clamp_cell(item.cell);
        occupied
            .entry(item.output.clone())
            .or_default()
            .insert(cell);
        result[slot] = Some((item.output.clone(), cell));
    }
    let fallback = state.default_output.clone();
    let grid = grid_for_output(state, &fallback);
    for (slot, _) in items
        .iter()
        .enumerate()
        .filter(|(_, item)| !state.output_grids.contains_key(&item.output))
    {
        let cells = occupied.entry(fallback.clone()).or_default();
        let cell = flamewm_desktop_core::model::first_free(cells, grid);
        cells.insert(cell);
        result[slot] = Some((fallback.clone(), cell));
    }
    result
        .into_iter()
        .map(|entry| entry.expect("every model item is projected"))
        .collect()
}

fn panel_geometries() -> Result<Vec<(OutputId, Rect)>, String> {
    let client = ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))?;
    match client.call_with_timeout(
        &ControlRequest::GetPanels,
        std::time::Duration::from_secs(2),
    ) {
        Ok(ControlResponse::Panels(snapshot)) => Ok(snapshot
            .panels
            .into_iter()
            .filter(|panel| panel.visible)
            .map(|panel| (panel.output, panel.geometry))
            .collect()),
        Ok(response) => Err(format!("GetPanels returned {response:?}")),
        Err(error) => Err(error.message.clone()),
    }
}

pub(super) fn subtract_panel_strip(mut area: Rect, panel: Rect) -> Rect {
    let Some(intersection) = area.intersection(panel) else {
        return area;
    };
    if intersection.width >= area.width.saturating_sub(16) {
        if intersection.y <= area.y + area.height / 2 {
            let bottom = intersection.y + intersection.height;
            let shift = bottom - area.y;
            if shift > 0 && shift < area.height {
                area.y = bottom;
                area.height -= shift;
            }
        } else {
            let height = intersection.y - area.y;
            if height > 0 {
                area.height = height;
            }
        }
        return area;
    }
    if intersection.height >= area.height.saturating_sub(16) {
        if intersection.x <= area.x + area.width / 2 {
            let right = intersection.x + intersection.width;
            let shift = right - area.x;
            if shift > 0 && shift < area.width {
                area.x = right;
                area.width -= shift;
            }
        } else {
            let width = intersection.x - area.x;
            if width > 0 {
                area.width = width;
            }
        }
    }
    area
}
