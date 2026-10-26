use super::*;
use crate::entry::settings_values::{accent_color, percent_alpha, setting_percent};
use flamewm_api::root_rubber::RootRubber;

/// One model item's rubber target, frozen at gesture start. `slot` is the visual node id
/// (`desktop-item-{index}`), formatted once here instead of on every toggle.
pub(super) struct RubberItem {
    slot: String,
    id: String,
    rect: Rect,
}

/// Projects every item once; the rubber gesture then tests against these frozen rects.
pub(super) fn rubber_snapshot(state: &DesktopState) -> Vec<RubberItem> {
    let cells = projected_cells(state);
    state
        .model
        .items()
        .zip(&cells)
        .enumerate()
        .map(|(index, (item, (output, cell)))| {
            let grid = grid_for_output(state, output);
            let point = grid.cell_to_pixel(*cell);
            RubberItem {
                slot: format!("desktop-item-{index}"),
                id: item.id.clone(),
                rect: Rect::new(point.x, point.y, grid.cell_width, grid.cell_height),
            }
        })
        .collect()
}

pub(super) fn rubber_bounds(state: &DesktopState) -> Rect {
    state
        .selection_output
        .as_ref()
        .map_or(state.grid.work_area, |output| {
            grid_for_output(state, output).work_area
        })
}

/// Deselects ids that no longer have a frozen rubber item (the item was removed mid-gesture).
pub(super) fn drop_missing_selected(selection: &mut SelectionModel, items: &[RubberItem]) {
    let live: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
    let stale: Vec<String> = selection
        .selected()
        .iter()
        .filter(|id| !live.contains(id.as_str()))
        .cloned()
        .collect();
    for id in &stale {
        selection.deselect(id);
    }
}

pub(super) fn end_rubber_gesture(state: &mut DesktopState) {
    state.selection_start = None;
    state.selection_output = None;
    state.selection.clear_rubber();
    state.rubber_items.clear();
}

/// Moves the selection to `rubber`'s hit set by flipping only the items whose membership
/// changed (symmetric difference against the current selection). `on_toggle(slot, selected)`
/// runs once per changed item and is the only place visuals are mutated.
fn diff_rubber_hits(
    items: &[RubberItem],
    selection: &mut SelectionModel,
    rubber: Rect,
    mut on_toggle: impl FnMut(&str, bool) -> Result<(), String>,
) -> Result<(), String> {
    for item in items {
        let hit = item.rect.intersects(rubber);
        if hit == selection.selected().contains(&item.id) {
            continue;
        }
        if hit {
            selection.select(item.id.clone());
        } else {
            selection.deselect(&item.id);
        }
        on_toggle(&item.slot, hit)?;
    }
    Ok(())
}

pub(super) fn apply_rubber_hits(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    rubber: Rect,
) -> Result<(), String> {
    // Style is resolved only for items that actually toggle, not once per motion.
    let settings = &state.settings;
    diff_rubber_hits(
        &state.rubber_items,
        &mut state.selection,
        rubber,
        |slot, selected| {
            let (fill_alpha, accent) = selection_style(settings);
            sync_item_selection(document, slot, selected, fill_alpha, accent)
        },
    )
}

fn selection_style(settings: &SettingsSnapshot) -> (u8, UiColor) {
    (
        percent_alpha(setting_percent(settings, "DesktopSelectionFillOpacity", 20)),
        accent_color(settings),
    )
}

fn sync_item_selection(
    document: &mut impl UiDocumentAccess,
    slot: &str,
    selected: bool,
    fill_alpha: u8,
    accent: UiColor,
) -> Result<(), String> {
    if selected {
        document.background(
            slot,
            UiColor {
                a: fill_alpha,
                ..accent
            },
        )?;
        document.border(slot, UiColor { a: 0xdd, ..accent })
    } else {
        document.background_clear(slot)?;
        document.border_clear(slot)
    }
}

pub(super) fn sync_selection_visuals(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let (fill_alpha, accent) = selection_style(&state.settings);
    for (index, item) in state.model.items().enumerate() {
        let selected = state.selection.selected().contains(&item.id);
        let slot = format!("desktop-item-{index}");
        sync_item_selection(document, &slot, selected, fill_alpha, accent)?;
    }
    Ok(())
}

/// Shows `overlay` at its rect, or hides the overlay for `None`. Setters run only on change.
pub(super) fn set_selection_overlay(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    overlay: Option<Rect>,
) -> Result<(), String> {
    if state.selection_overlay == overlay {
        return Ok(());
    }
    match overlay {
        Some(rect) => {
            if state.selection_overlay.is_none() {
                document.visible("desktop-selection", true)?;
            }
            document.position("desktop-selection", rect.x as f32, rect.y as f32)?;
            document.size("desktop-selection", rect.width as f32, rect.height as f32)?;
        }
        None => document.visible("desktop-selection", false)?,
    }
    state.selection_overlay = overlay;
    publish_root_rubber(state, overlay)
}

/// Mirrors the live rubber onto the root window so the WM can recompose the frame shadows it
/// crosses (the rubber itself sits below them). A desktop without a root writer publishes nothing.
fn publish_root_rubber(state: &mut DesktopState, overlay: Option<Rect>) -> Result<(), String> {
    let (fill_alpha, accent) = selection_style(&state.settings);
    let Some(publisher) = state.rubber_publisher.as_mut() else {
        return Ok(());
    };
    let rubber = overlay.map(|rect| root_rubber_payload(rect, fill_alpha, accent).to_cardinals());
    publisher.set(rubber.as_ref().map(|values| values.as_slice()))
}

/// The root-space description of the overlay the desktop is painting at `rect`.
pub(super) fn root_rubber_payload(rect: Rect, fill_alpha: u8, accent: UiColor) -> RootRubber {
    RootRubber {
        x: rect.x,
        y: rect.y,
        width: rect.width.max(0) as u32,
        height: rect.height.max(0) as u32,
        fill: [accent.r, accent.g, accent.b, fill_alpha],
        border: [accent.r, accent.g, accent.b, 0xdd],
    }
}

pub(super) fn normalized_selection_rect(start: Point, end: Point, bounds: Rect) -> Rect {
    let start_x = start.x.clamp(bounds.x, bounds.right().saturating_sub(1));
    let start_y = start.y.clamp(bounds.y, bounds.bottom().saturating_sub(1));
    let end_x = end.x.clamp(bounds.x, bounds.right().saturating_sub(1));
    let end_y = end.y.clamp(bounds.y, bounds.bottom().saturating_sub(1));
    Rect::new(
        start_x.min(end_x),
        start_y.min(end_y),
        (start_x - end_x).unsigned_abs().max(1) as i32,
        (start_y - end_y).unsigned_abs().max(1) as i32,
    )
}

#[cfg(test)]
mod rubber_tests {
    use super::*;

    fn grid_items() -> Vec<RubberItem> {
        (0..100)
            .map(|i| RubberItem {
                slot: format!("desktop-item-{i}"),
                id: format!("item-{i:03}"),
                rect: Rect::new((i % 10) as i32 * 20, (i / 10) as i32 * 20, 16, 16),
            })
            .collect()
    }

    #[test]
    fn rubber_motion_matches_full_hit_test_and_toggles_only_changed_items() {
        let items = grid_items();
        let mut selection = SelectionModel::default();
        let (mut toggles, mut expected_toggles) = (0usize, 0usize);
        let mut no_op_toggles = 0usize;

        // 100 no-op motions at one rect: the hit set is stable, so nothing toggles.
        let still = Rect::new(0, 0, 50, 50);
        diff_rubber_hits(&items, &mut selection, still, |_, _| Ok(())).expect("settle");
        let mut previous = selection.selected().clone();
        for _ in 0..100 {
            diff_rubber_hits(&items, &mut selection, still, |_, _| {
                no_op_toggles += 1;
                Ok(())
            })
            .expect("no-op step");
        }
        assert_eq!(no_op_toggles, 0);

        // 100 moving steps: the sweep crosses items in and out of the rubber band.
        for step in 0..100 {
            let rubber = Rect::new(step * 2, 0, 50, 200);
            let mut reference = SelectionModel::default();
            reference.set_rubber(rubber);
            let expected = reference.hit_test(items.iter().map(|item| (item.id.as_str(), item.rect)));
            expected_toggles += expected.symmetric_difference(&previous).count();
            previous = expected.clone();

            diff_rubber_hits(&items, &mut selection, rubber, |_, _| {
                toggles += 1;
                Ok(())
            })
            .expect("moving step");
            assert_eq!(selection.selected(), &expected, "step {step}");
        }
        assert_eq!(toggles, expected_toggles);
        assert!(toggles > 0);
        eprintln!("BENCH rubber_motion_toggles={toggles} rubber_motion_expected_toggles={expected_toggles} rubber_noop_toggles={no_op_toggles}");
    }

    #[test]
    fn rubber_motion_does_not_project_cells_per_motion() {
        let items = grid_items();
        let mut selection = SelectionModel::default();
        let before = PROJECTED_CELLS_CALLS.with(std::cell::Cell::get);
        for step in 0..100 {
            let rubber = Rect::new(step * 2, 0, 50, 200);
            diff_rubber_hits(&items, &mut selection, rubber, |_, _| Ok(())).expect("motion");
        }
        assert_eq!(PROJECTED_CELLS_CALLS.with(std::cell::Cell::get), before);
    }

    #[test]
    fn root_rubber_payload_mirrors_overlay_rect_and_accent() {
        let accent = UiColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
            a: 0xff,
        };
        let payload = root_rubber_payload(Rect::new(-5, 7, 90, 40), 0x33, accent);
        assert_eq!((payload.x, payload.y), (-5, 7));
        assert_eq!((payload.width, payload.height), (90, 40));
        assert_eq!(payload.fill, [0x10, 0x20, 0x30, 0x33]);
        assert_eq!(payload.border, [0x10, 0x20, 0x30, 0xdd]);
    }

    #[test]
    fn rubber_refresh_drops_selection_of_items_removed_mid_gesture() {
        let items = grid_items();
        let mut selection = SelectionModel::default();
        selection.select("item-001");
        selection.select("item-002");
        selection.select("removed-item");
        // The refreshed snapshot no longer contains "removed-item".
        drop_missing_selected(&mut selection, &items);
        assert_eq!(
            selection.selected().iter().map(String::as_str).collect::<Vec<_>>(),
            ["item-001", "item-002"]
        );
    }
}
