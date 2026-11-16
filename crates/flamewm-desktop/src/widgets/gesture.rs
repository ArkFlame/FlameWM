use super::*;

/// Pixel rectangle after dragging only the `edges` of `original` by (`dx`, `dy`). Opposite
/// edges stay put; each side keeps at least one pixel and the result stays inside `bounds`.
pub(super) fn edge_drag_rect(original: Rect, edges: Edges, dx: i32, dy: i32, bounds: Rect) -> Rect {
    let (mut left, mut top) = (original.x, original.y);
    let (mut right, mut bottom) = (original.right(), original.bottom());
    if edges.left {
        left = (left + dx).min(right - 1);
    }
    if edges.right {
        right = (right + dx).max(left + 1);
    }
    if edges.top {
        top = (top + dy).min(bottom - 1);
    }
    if edges.bottom {
        bottom = (bottom + dy).max(top + 1);
    }
    Rect::new(left, top, right - left, bottom - top).clamp_inside(bounds)
}

/// Shows the drag ghost over `target`'s cell, accent-tinted when the cell is a valid
/// landing and error-tinted when nothing fits, or hides it.
pub(super) fn sync_ghost(
    document: &mut impl UiDocumentAccess,
    metrics: GridMetrics,
    accent: Rgb,
    target: Option<(Placement, bool)>,
) -> Result<(), String> {
    match target {
        Some((cell, valid)) => {
            let rect = metrics.rect_of(cell);
            let tint = if valid { accent } else { danger_rgb() };
            document.position("widget-ghost", rect.x as f32, rect.y as f32)?;
            document.size("widget-ghost", rect.width as f32, rect.height as f32)?;
            document.border("widget-ghost", color(tint, 0xff))?;
            document.visible("widget-ghost", true)
        }
        None => document.visible("widget-ghost", false),
    }
}

pub(super) fn danger_rgb() -> Rgb {
    let value = Color::DANGER.0;
    Rgb::new((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Shows one outline per grid cell across the whole work area. Called once when a drag
/// starts; idle desktops never run it.
pub(super) fn show_slot_overlay(
    document: &mut impl UiDocumentAccess,
    metrics: GridMetrics,
) -> Result<(), String> {
    for index in 0..WIDGET_OVERLAY_CELLS {
        let id = format!("widget-slot-{index}");
        let col = (index % WIDGET_OVERLAY_COLS) as i32;
        let row = (index / WIDGET_OVERLAY_COLS) as i32;
        if col < metrics.cols && row < metrics.rows {
            let rect = metrics.rect_of(Placement::left(col, row, Span::SINGLE));
            document.position(&id, rect.x as f32, rect.y as f32)?;
            document.size(&id, rect.width as f32, rect.height as f32)?;
            document.visible(&id, true)?;
        } else {
            document.visible(&id, false)?;
        }
    }
    Ok(())
}

/// Cell a live gesture would land on, with whether it is a valid landing. A cell that is
/// free (or holds a same-size swap partner) is valid. When the snapped cell collides and
/// a nearer free cell exists, the ghost moves there and is valid. When nothing fits, the
/// ghost stays on the snapped cell, clamped inside the grid, and is invalid.
pub(super) fn gesture_ghost(
    state: &DesktopState,
    id: u32,
    mode: GestureMode,
    rect: Rect,
    metrics: GridMetrics,
) -> Option<(Placement, bool)> {
    let widget = state.widgets.store.get(id)?;
    let current = widget.grid?;
    match mode {
        GestureMode::Move => {
            let snapped = metrics.snap(rect.origin(), current.span);
            match state.widgets.store.preview_drop(id, rect.origin(), metrics) {
                Some(cell) => Some((cell, true)),
                None => Some((snapped, false)),
            }
        }
        GestureMode::Resize(edges) => {
            let want = widget_grid::edge_resize(widget.kind, current, edges, rect, metrics);
            match state.widgets.store.preview_placement(id, want, metrics) {
                Some(cell) => Some((cell, true)),
                None => Some((want, false)),
            }
        }
    }
}

pub(super) fn hide_slot_overlay(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for index in 0..WIDGET_OVERLAY_CELLS {
        document.visible(&format!("widget-slot-{index}"), false)?;
    }
    Ok(())
}

/// Widget grid for the current work area and UI scale.
pub(crate) fn widget_metrics(state: &DesktopState) -> GridMetrics {
    GridMetrics::for_work_area(widget_bounds(state), i32::from(state.grid.scale_percent))
}

/// Work area the widget grid covers: the primary output (`state.grid` follows
/// `default_output`), never a union of every output.
pub(crate) fn widget_bounds(state: &DesktopState) -> Rect {
    state.grid.work_area
}

#[must_use]
/// Layer for a widget slot: lifted above sibling cards while a drag is live.
pub(super) fn dragged_layer(released: bool) -> UiLayer {
    if released {
        UiLayer::Content
    } else {
        UiLayer::Floating
    }
}

/// Applies the latest coalesced drag motion. Called once per event flush from `main.rs`.
pub(crate) fn apply_pending_drag(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(pointer) = state.widgets.pending_pointer.take() else {
        return Ok(());
    };
    let Some(gesture) = state.widgets.gesture else {
        return Ok(());
    };
    drag_to(document, state, gesture, pointer, false)
}

/// Moves a live gesture to `pointer`. A motion previews only: ghost outline, slot outlines
/// and the fade/lift of the dragged card. Release commits the snapped rect and clears it.
pub(super) fn drag_to(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mut gesture: Gesture,
    pointer: (f32, f32),
    release: bool,
) -> Result<(), String> {
    let id = gesture.widget;
    let dx = pointer.0 - gesture.start_x;
    let dy = pointer.1 - gesture.start_y;
    let was_moved = gesture.moved;
    gesture.moved |= dx * dx + dy * dy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;
    state.widgets.gesture = (!release).then_some(gesture);
    if !gesture.moved {
        return Ok(());
    }
    let bounds = widget_bounds(state);
    let rect = match gesture.mode {
        GestureMode::Move => move_widget_rect(gesture.original, dx as i32, dy as i32, bounds),
        GestureMode::Resize(edges) => {
            edge_drag_rect(gesture.original, edges, dx as i32, dy as i32, bounds)
        }
    };
    // The widget stays put during the drag; only the preview zone moves. Release snaps the
    // widget to the grid.
    if release {
        state.widgets.store.set_rect(id, rect);
        let metrics = widget_metrics(state);
        match gesture.mode {
            GestureMode::Move => {
                state.widgets.store.drop_on_grid(id, rect.origin(), metrics);
            }
            GestureMode::Resize(edges) => {
                if let Some((kind, current)) = state
                    .widgets
                    .store
                    .get(id)
                    .and_then(|w| w.grid.map(|g| (w.kind, g)))
                {
                    let want = widget_grid::edge_resize(kind, current, edges, rect, metrics);
                    state.widgets.store.place_nearest(id, want, metrics);
                }
            }
        }
        state.widgets.persist();
    }
    state.widgets.store.anchor_to(id, bounds);
    // Slot outlines exist only while a drag or resize is live.
    if release {
        if state.widgets.slot_overlay {
            hide_slot_overlay(document)?;
            state.widgets.slot_overlay = false;
        }
    } else if !state.widgets.slot_overlay {
        show_slot_overlay(document, widget_metrics(state))?;
        state.widgets.slot_overlay = true;
    }
    // Ghost: the cell this gesture would land on, hidden once released.
    let metrics = widget_metrics(state);
    let ghost = if release {
        None
    } else {
        gesture_ghost(state, id, gesture.mode, rect, metrics)
    };
    let accent = projection_accent(state);
    sync_ghost(document, metrics, accent, ghost)?;
    // The dragged card fades and lifts once per gesture and restores on release.
    if let Some(slot) = state
        .widgets
        .store
        .widgets()
        .iter()
        .position(|w| w.id == id)
        && (!was_moved || release)
    {
        let node = format!("widget-{}", slot + 1);
        document.opacity(&node, if release { 1.0 } else { DRAG_OPACITY })?;
        document.layer(&node, dragged_layer(release))?;
    }
    // Slot content is unchanged until release, so motion does not rebuild it.
    if release {
        sync_widgets(document, state)?;
    }
    Ok(())
}
