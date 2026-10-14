//! Desktop sticky-note owner: note state, persistence debounce, visual
//! projection, text edit, move/resize gestures and sticky menu actions.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use flamewm_api::{OutputId, Point, Rect};
use flamewm_desktop_core::sticky::{
    ResizeCorner, StickyNote, StickyNoteStore, move_rect, resize_rect,
};
use flamewm_desktop_core::sticky_persistence;
use flamewm_profiler::CounterPoint;
use flamewm_reactor::Reactor;
use flamewm_ui_x11::{PointerButton, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess};

use crate::projection;
use crate::{
    DesktopState, MenuKind, MenuState, context_secondary_counter, hide_confirm, hide_menus,
    pointer_button, pointer_point, sync_menu,
};

const STICKY_SLOTS: usize = projection::STICKY_SLOT_COUNT;

fn sticky_edit_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.edit"))
}

fn sticky_move_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.move"))
}

fn sticky_resize_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.resize"))
}

fn sticky_persist_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.persist"))
}

/// All sticky-note state owned by the desktop process.
pub(crate) struct StickyState {
    pub(crate) store: StickyNoteStore,
    path: PathBuf,
    pub(crate) notes_enabled: bool,
    edit: Option<StickyEdit>,
    gesture: Option<StickyGesture>,
    persist_pending: bool,
    persist_armed: bool,
    persist_due: Option<Instant>,
}

impl StickyState {
    pub(crate) fn new(store: StickyNoteStore, path: PathBuf, notes_enabled: bool) -> Self {
        Self {
            store,
            path,
            notes_enabled,
            edit: None,
            gesture: None,
            persist_pending: false,
            persist_armed: false,
            persist_due: None,
        }
    }
}

#[derive(Clone)]
struct StickyEdit {
    note_id: String,
    buffer: String,
}

#[derive(Clone)]
struct StickyGesture {
    note_id: String,
    mode: StickyGestureMode,
    start_x: f32,
    start_y: f32,
    original: Rect,
    /// Set once the pointer has moved past STICKY_DRAG_THRESHOLD_PX.
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StickyGestureMode {
    Move,
    Resize,
}

pub(crate) fn register_persist_timer(reactor: &mut Reactor) -> Result<(), String> {
    // Debounce owner is the reactor tick: the flag is armed by mutations and
    // flushed in poll_persist/flush_persist_if_due (<=500ms via
    // a 100ms repeat). No threads.
    reactor
        .register_timer(Duration::from_millis(100), true, || {})
        .map(|_| ())
        .map_err(|error| format!("register sticky persist timer: {error}"))
}

pub(crate) fn poll_persist(
    _document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if state.sticky.persist_pending && !state.sticky.persist_armed {
        state.sticky.persist_armed = true;
        state.sticky.persist_due = Some(Instant::now() + Duration::from_millis(500));
    }
    Ok(())
}

pub(crate) fn flush_persist_if_due(
    _document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(due) = state.sticky.persist_due else {
        return Ok(());
    };
    if Instant::now() < due {
        return Ok(());
    }
    state.sticky.persist_due = None;
    state.sticky.persist_pending = false;
    state.sticky.persist_armed = false;
    persist(state)
}

fn persist(state: &mut DesktopState) -> Result<(), String> {
    sticky_persist_counter().increment();
    sticky_persistence::persist(&state.sticky.path, &state.sticky.store)
        .map_err(|error| format!("{error:?}"))
}

fn schedule_persist(state: &mut DesktopState) {
    state.sticky.persist_pending = true;
}

pub(crate) fn flush_now(state: &DesktopState) {
    if state.sticky.persist_pending {
        let _ = sticky_persistence::persist(&state.sticky.path, &state.sticky.store);
    }
}

fn visible_sticky_notes(state: &DesktopState) -> Vec<StickyNote> {
    state
        .sticky
        .store
        .notes_for_workspace(state.workspace.active_index())
        .into_iter()
        .take(STICKY_SLOTS)
        .cloned()
        .collect()
}

pub(crate) fn sync_sticky(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let visible = visible_sticky_notes(state);
    let show_any =
        state.sticky.notes_enabled && state.sticky.store.enabled() && !visible.is_empty();
    for slot in 0..STICKY_SLOTS {
        let number = slot + 1;
        let id = format!("sticky-{number}");
        if let Some(note) = visible.get(slot) {
            document.visible(&id, show_any)?;
            document.position(&id, note.rect.x as f32, note.rect.y as f32)?;
            document.size(&id, note.rect.width as f32, note.rect.height as f32)?;
            document.background(
                &id,
                UiColor {
                    r: note.background.red,
                    g: note.background.green,
                    b: note.background.blue,
                    a: 255,
                },
            )?;
            let number = slot + 1;
            let text_id = format!("sticky-{number}-text");
            let editing = state
                .sticky
                .edit
                .as_ref()
                .filter(|edit| edit.note_id == note.id)
                .map(|edit| edit.buffer.clone());
            let text = match editing {
                Some(buffer) => projection::caret_text(&buffer, true),
                None => note.text.clone(),
            };
            document.text(&text_id, text)?;
            document.font_size(&text_id, note.text_size as f32)?;
            document.font_weight(&text_id, if note.bold { 700 } else { 400 })?;
            document.foreground(
                &text_id,
                UiColor {
                    r: note.foreground.red,
                    g: note.foreground.green,
                    b: note.foreground.blue,
                    a: 255,
                },
            )?;
            let number = slot + 1;
            document.visible(&format!("sticky-{number}-drag"), show_any)?;
            for corner in ["nw", "ne", "sw", "se"] {
                document.visible(&format!("sticky-{number}-resize-{corner}"), show_any)?;
            }
        } else {
            document.visible(&id, false)?;
            let number = slot + 1;
            document.text(&format!("sticky-{number}-text"), String::new())?;
            let _ = document.visible(&format!("sticky-{number}-drag"), false);
            for corner in ["nw", "ne", "sw", "se"] {
                let _ = document.visible(&format!("sticky-{number}-resize-{corner}"), false);
            }
        }
    }
    Ok(())
}

pub(crate) fn is_editing(state: &DesktopState) -> bool {
    state.sticky.edit.is_some()
}

pub(crate) fn commit_edit(state: &mut DesktopState) {
    if state.sticky.edit.take().is_some() {
        schedule_persist(state);
    }
}

/// Transient-cancel path: commits any active edit and drops an in-flight gesture.
pub(crate) fn cancel_edit_and_gesture(state: &mut DesktopState) {
    commit_edit(state);
    state.sticky.gesture = None;
}

/// Creates a note at `anchor` on the active workspace, respecting the slot cap.
pub(crate) fn create_note(state: &mut DesktopState, anchor: Point) {
    if state.sticky.store.enabled() {
        let workspace = state.workspace.active_index();
        let sequence = state.sticky.store.count_for_workspace(workspace) + 1;
        let id = format!("sticky-{workspace}-{sequence}");
        if state.sticky.store.count_for_workspace(workspace) < STICKY_SLOTS {
            state.sticky.store.create(StickyNote::new(
                id,
                OutputId::new("default"),
                workspace,
                anchor.x,
                anchor.y,
            ));
            schedule_persist(state);
        }
    }
}

/// Applies a sticky menu action (delete, colour preset, bold) to the selected note.
pub(crate) fn apply_menu_action(action: &str, state: &mut DesktopState) -> Result<(), String> {
    match action {
        "sticky.menu.delete" => {
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone())
                .or_else(|| {
                    state
                        .sticky
                        .store
                        .notes()
                        .first()
                        .map(|note| note.id.clone())
                });
            if let Some(note_id) = note_id {
                state.sticky.store.remove(&note_id);
                if state
                    .sticky
                    .edit
                    .as_ref()
                    .is_some_and(|edit| edit.note_id == note_id)
                {
                    state.sticky.edit = None;
                }
                sticky_persist_counter().increment();
                sticky_persistence::persist(&state.sticky.path, &state.sticky.store)
                    .map_err(|error| format!("{error:?}"))?;
            }
        }
        "sticky.menu.yellow" | "sticky.menu.green" | "sticky.menu.pink" | "sticky.menu.blue" => {
            let preset = match action {
                "sticky.menu.yellow" => 0,
                "sticky.menu.green" => 1,
                "sticky.menu.pink" => 2,
                _ => 3,
            };
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone());
            if let Some(note_id) = note_id {
                state.sticky.store.set_color(&note_id, preset);
                schedule_persist(state);
            }
        }
        "sticky.menu.bold" => {
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone());
            if let Some(note_id) = note_id {
                state.sticky.store.toggle_bold(&note_id);
                schedule_persist(state);
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn handle_text_input(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(note_id) = state.sticky.edit.as_ref().map(|edit| edit.note_id.clone()) else {
        return Ok(());
    };
    if action.text.as_deref() == Some("") {
        state.sticky.edit = None;
        return sync_sticky(document, state);
    }
    if let Some(text) = action.text.clone() {
        if text == "\u{8}" || text == "\u{7f}" {
            if let Some(current) = state.sticky.edit.as_mut() {
                current.buffer.pop();
            }
        } else {
            for ch in text.chars() {
                if ch == '\n' || !ch.is_control() {
                    if let Some(current) = state.sticky.edit.as_mut() {
                        current.buffer.push(ch);
                    }
                }
            }
        }
        if let Some(buffer) = state
            .sticky
            .edit
            .as_ref()
            .map(|current| current.buffer.clone())
        {
            state.sticky.store.set_text(&note_id, buffer);
            sticky_edit_counter().increment();
            schedule_persist(state);
        }
        return sync_sticky(document, state);
    }
    Ok(())
}

/// Routes every `sticky.*` UI action from the desktop event loop.
pub(crate) fn handle_action(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    // Sticky Secondary opens the sticky context menu for the exact note under
    // the pointer. It routes BEFORE the generic primary dispatch below;
    // otherwise the generic branch shadows it and it is unreachable.
    if action.phase == UiActionPhase::Release && pointer_button(action) == PointerButton::Secondary
    {
        return open_context_menu(action, document, state);
    }
    if action.action == "sticky.move" || action.action == "sticky.menu.surface" {
        return Ok(());
    }
    handle_slot(action, document, state)
}

fn open_context_menu(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let visible = visible_sticky_notes(state);
    let slot_index = slot_number(&action.action).and_then(|number| number.checked_sub(1));
    if let Some(position) = slot_index {
        if let Some(note) = visible.get(position) {
            context_secondary_counter().increment();
            let anchor = pointer_point(action, state.grid.work_area);
            state.menu = Some(MenuState {
                kind: MenuKind::Sticky,
                anchor,
                selected_id: Some(note.id.clone()),
            });
            state.pending_delete = None;
            hide_confirm(document)?;
            return sync_menu(document, state);
        }
    }
    Ok(())
}

fn handle_slot(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let visible = visible_sticky_notes(state);
    let slot_index = slot_number(&action.action).and_then(|number| number.checked_sub(1));
    let Some(position) = slot_index else {
        return Ok(());
    };
    let Some(note) = visible.get(position).cloned() else {
        return Ok(());
    };
    let note_id = note.id.clone();
    // Primary press on the note body (.move), drag overlay (.drag) or text
    // (.text) activates the edit buffer and records a pending move. The move
    // only starts once the pointer passes STICKY_DRAG_THRESHOLD_PX; a release
    // below the threshold is a plain click and keeps the edit active.
    if is_note_body_action(&action.action) {
        if pointer_button(action) != PointerButton::Primary {
            return Ok(());
        }
        match action.phase {
            UiActionPhase::Press => {
                hide_menus(document)?;
                state.menu = None;
                state.sticky.edit = Some(StickyEdit {
                    note_id: note_id.clone(),
                    buffer: note.text.clone(),
                });
                state.sticky.gesture = Some(StickyGesture {
                    note_id,
                    mode: StickyGestureMode::Move,
                    start_x: action.x,
                    start_y: action.y,
                    original: note.rect,
                    moved: false,
                });
                sticky_edit_counter().increment();
                return sync_sticky(document, state);
            }
            UiActionPhase::Motion | UiActionPhase::Release => {
                let release = action.phase == UiActionPhase::Release;
                let Some(gesture) = state.sticky.gesture.as_mut() else {
                    return Ok(());
                };
                if gesture.note_id != note_id || gesture.mode != StickyGestureMode::Move {
                    return Ok(());
                }
                let dx = action.x - gesture.start_x;
                let dy = action.y - gesture.start_y;
                gesture.moved |= sticky_drag_exceeded(dx, dy);
                let moved = gesture.moved;
                let original = gesture.original;
                if release {
                    state.sticky.gesture = None;
                }
                if !moved {
                    return Ok(());
                }
                let rect = move_rect(original, dx as i32, dy as i32, state.grid.work_area);
                state.sticky.store.set_rect(&note_id, rect);
                sticky_move_counter().increment();
                if release {
                    flush_now(state);
                }
                return sync_sticky(document, state);
            }
            UiActionPhase::Hover => {}
        }
        return Ok(());
    }
    if action.action.contains(".resize-") {
        if pointer_button(action) != PointerButton::Primary {
            return Ok(());
        }
        let corner = if action.action.ends_with("-nw") {
            ResizeCorner::TopLeft
        } else if action.action.ends_with("-ne") {
            ResizeCorner::TopRight
        } else if action.action.ends_with("-sw") {
            ResizeCorner::BottomLeft
        } else {
            ResizeCorner::BottomRight
        };
        match action.phase {
            UiActionPhase::Press => {
                state.sticky.gesture = Some(StickyGesture {
                    note_id,
                    mode: StickyGestureMode::Resize,
                    start_x: action.x,
                    start_y: action.y,
                    original: note.rect,
                    moved: false,
                });
                state.sticky.gesture.as_mut().map(|gesture| {
                    gesture.mode = StickyGestureMode::Resize;
                });
            }
            UiActionPhase::Motion | UiActionPhase::Release => {
                if let Some(gesture) = state.sticky.gesture.clone() {
                    if gesture.note_id == note_id {
                        let rect = resize_rect(
                            gesture.original,
                            corner,
                            (action.x - gesture.start_x) as i32,
                            (action.y - gesture.start_y) as i32,
                            state.grid.work_area,
                        );
                        state.sticky.store.set_rect(&note_id, rect);
                        sticky_resize_counter().increment();
                        if action.phase == UiActionPhase::Release {
                            state.sticky.gesture = None;
                            flush_now(state);
                        }
                        sync_sticky(document, state)?;
                    }
                }
            }
            UiActionPhase::Hover => {}
        }
        return Ok(());
    }
    Ok(())
}

fn slot_number(action: &str) -> Option<usize> {
    action
        .strip_prefix("sticky.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|number| number.parse::<usize>().ok())
}

/// Pointer travel (logical px) before a note press becomes a move.
const STICKY_DRAG_THRESHOLD_PX: f32 = 5.0;

fn is_note_body_action(action: &str) -> bool {
    action.ends_with(".move") || action.ends_with(".drag") || action.ends_with(".text")
}

fn sticky_drag_exceeded(dx: f32, dy: f32) -> bool {
    dx * dx + dy * dy >= STICKY_DRAG_THRESHOLD_PX * STICKY_DRAG_THRESHOLD_PX
}

#[cfg(test)]
mod tests {
    use super::{is_note_body_action, sticky_drag_exceeded};

    #[test]
    fn click_below_threshold_stays_a_click() {
        assert!(!sticky_drag_exceeded(0.0, 0.0));
        assert!(!sticky_drag_exceeded(3.0, 3.0));
        assert!(!sticky_drag_exceeded(-4.9, 0.0));
    }

    #[test]
    fn movement_at_or_past_threshold_is_a_drag() {
        assert!(sticky_drag_exceeded(0.0, 5.0));
        assert!(sticky_drag_exceeded(-5.0, 0.0));
        assert!(sticky_drag_exceeded(3.0, 4.0));
    }

    #[test]
    fn body_title_text_and_drag_actions_start_moves_but_controls_do_not() {
        assert!(is_note_body_action("sticky.1.move"));
        assert!(is_note_body_action("sticky.16.drag"));
        assert!(is_note_body_action("sticky.2.text"));
        assert!(!is_note_body_action("sticky.2.resize-se"));
        assert!(!is_note_body_action("sticky.2.menu"));
    }
}
