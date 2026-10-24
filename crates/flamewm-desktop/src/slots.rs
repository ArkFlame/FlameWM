use super::*;

/// Projects every slot from the model. A tile whose inputs equal its last projection only
/// re-asserts visibility; an empty slot is cleared once and then skipped on later syncs.
pub(super) fn project_slots(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let items: Vec<_> = state.model.items().take(SLOT_COUNT).collect();
    let cache = match state.projected_cache.take() {
        Some(cache) if cache.matches(state) => cache,
        _ => ProjectedCellsCache::capture(state),
    };
    let cells = &cache.cells;
    let generation = projection::theme_generation();
    for index in 0..SLOT_COUNT {
        let item = items.get(index).copied();
        if let Some(item) = item {
            let (output, cell) = &cells[index];
            let output_grid = grid_for_output(state, output);
            let point = output_grid.cell_to_pixel(*cell);
            let kind = item.kind;
            let icon = launcher_icon(state, kind, &item.path);
            let label = desktop_item_label(state, item);
            let scale = output_grid.visual_scale_percent();
            if state.slot_inputs[index].as_ref().is_some_and(|held| {
                held.matches(kind, label, point, icon.as_deref(), scale, generation)
            }) {
                // Unchanged tile: projection would only re-assert visibility. Drag
                // hides sources without touching model state, so restore it here.
                document.visible(&format!("desktop-item-{index}"), true)?;
                continue;
            }
            let inputs = SlotInputs {
                kind,
                label: label.to_owned(),
                point,
                icon,
                scale,
                generation,
            };
            {
                let _icon = icon_point().start();
                let key = projection::project_item(
                    document,
                    &mut state.icons,
                    index,
                    inputs.kind,
                    &inputs.label,
                    inputs.point,
                    inputs.icon.as_deref(),
                    inputs.scale,
                )?;
                state.icon_slots[index] = Some(key);
            }
            state.slot_inputs[index] = Some(inputs);
        } else {
            if !empty_slot_needs_clear(state.slots_primed, state.slot_inputs[index].is_some()) {
                continue;
            }
            state.icon_slots[index] = None;
            state.slot_inputs[index] = None;
            projection::clear_item(document, index)?;
        }
    }
    state.projected_cache = Some(cache);
    state.slots_primed = true;
    Ok(())
}
/// Every projection input of one desktop tile; equal values yield identical document writes.
#[derive(Clone, PartialEq)]
pub(super) struct SlotInputs {
    kind: DesktopItemKind,
    label: String,
    point: Point,
    icon: Option<String>,
    scale: u16,
    generation: u64,
}

impl SlotInputs {
    /// Compares against borrowed inputs so an unchanged tile never allocates its label or icon.
    fn matches(
        &self,
        kind: DesktopItemKind,
        label: &str,
        point: Point,
        icon: Option<&str>,
        scale: u16,
        generation: u64,
    ) -> bool {
        self.kind == kind
            && self.label == label
            && self.point == point
            && self.icon.as_deref() == icon
            && self.scale == scale
            && self.generation == generation
    }
}

#[cfg(test)]
mod slot_input_tests {
    use super::*;

    #[test]
    fn empty_slots_clear_on_first_pass_then_only_when_they_held_a_tile() {
        let empty_pass = |primed: bool, held: [bool; 3]| {
            held.iter()
                .filter(|&&held_tile| empty_slot_needs_clear(primed, held_tile))
                .count()
        };
        assert_eq!(empty_pass(false, [false, false, false]), 3, "first pass clears every slot");
        assert_eq!(empty_pass(true, [false, false, false]), 0, "unchanged empty slots skip");
        assert_eq!(empty_pass(true, [false, true, false]), 1, "occupied-to-empty clears once");
    }

    fn tile() -> SlotInputs {
        SlotInputs {
            kind: DesktopItemKind::File,
            label: "notes.txt".to_owned(),
            point: Point::new(10, 20),
            icon: None,
            scale: 100,
            generation: 1,
        }
    }

    #[test]
    fn identical_tile_inputs_match_and_each_projection_input_change_mismatches() {
        let base = tile();
        assert!(base == tile(), "unchanged inputs must skip re-projection");
        assert!(base != SlotInputs { kind: DesktopItemKind::Directory, ..tile() });
        assert!(base != SlotInputs { label: "other".to_owned(), ..tile() });
        assert!(base != SlotInputs { point: Point::new(11, 20), ..tile() });
        assert!(base != SlotInputs { icon: Some("x".to_owned()), ..tile() });
        assert!(base != SlotInputs { scale: 150, ..tile() });
        assert!(base != SlotInputs { generation: 2, ..tile() }, "theme change must re-project");
    }

    #[test]
    fn stale_theme_generation_result_is_rejected_after_reprojection() {
        let current = projection::ParallelIconService::icon_key(DesktopItemKind::File, None);
        let mut stale = current.clone();
        stale.theme_generation = current.theme_generation.wrapping_add(1);
        assert!(slot_expects_key(Some(&current), &current));
        assert!(!slot_expects_key(Some(&current), &stale));
        assert!(!slot_expects_key(None, &current));
    }
}

/// Wake-driven icon completion: the IconService wake FD sets the pending
/// flag (no inotify/click dependency); this drains the service and applies
/// each result only to slots whose tracked [`IconKey`] still matches, then
/// redraws each affected tile surface exactly once via the document marks.
pub(super) fn apply_pending_icon_results(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.icons_pending.swap(false, Ordering::Acquire) {
        return Ok(());
    }
    apply_ready_icons(document, state)
}

/// An empty slot needs a clear until the first slot pass, and again whenever it held a tile.
fn empty_slot_needs_clear(primed: bool, held_tile: bool) -> bool {
    !primed || held_tile
}

/// A completed icon result applies only to the slot still expecting its exact key
/// (including theme generation), so results for a replaced theme are dropped.
fn slot_expects_key(slot: Option<&IconKey>, key: &IconKey) -> bool {
    slot.is_some_and(|expected| expected == key)
}

pub(super) fn apply_ready_icons(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let ready = state.icons.drain_ready();
    if ready.is_empty() {
        return Ok(());
    }
    for (key, raster) in ready {
        let Some(raster) = raster else { continue };
        for index in 0..SLOT_COUNT {
            let Some(expected) = state.icon_slots.get(index).cloned().flatten() else {
                continue;
            };
            let Some(kind) = state.model.items().nth(index).map(|item| item.kind) else {
                continue;
            };
            if slot_expects_key(Some(&expected), &key) {
                state
                    .icons
                    .apply_if_current(document, index, kind, &expected, &key, raster.clone())?;
            }
        }
    }
    // A live prediction shares the same async icon cache as real desktop tiles.
    if let Some(preview) = state.conversion_preview.as_ref() {
        let key = projection::ParallelIconService::icon_key(
            DesktopItemKind::File,
            Some(projection::file_icon_name(&preview.destination)),
        );
        if let Some(raster) = state.icons.cached(&key) {
            document.image_rgba8(
                "desktop-convert-preview-icon",
                flamewm_ui_x11::RuntimeImage {
                    source: raster.source,
                    width: raster.width,
                    height: raster.height,
                    pixels: raster.pixels,
                },
            )?;
        }
    }
    Ok(())
}
