use super::*;

impl X11App {
    pub(crate) unsafe fn event_loop<F>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        // No unconditional redraw on entry: presenters start from the
        // retained scene; damage/expose drive paints via redraw_if_dirty.
        let mut buffers = ActionBuffers::default();
        loop {
            let mut event = MaybeUninit::<XEvent>::uninit();
            // SAFETY: XNextEvent initializes the supplied XEvent storage before returning.
            unsafe { XNextEvent(self.display, event.as_mut_ptr()) };
            // SAFETY: XNextEvent wrote a complete XEvent into the MaybeUninit storage.
            let event = unsafe { event.assume_init() };
            // Root raw presses carry no window of ours; handle them before the window filter.
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            if unsafe { event.type_ } == GENERIC_EVENT {
                if Some(unsafe { generic_cookie(&event) }.extension)
                    == self.press_selection.opcode()
                {
                    // SAFETY: the event is a GenericEvent of the selected XI extension.
                    unsafe { self.record_raw_press(&event, document.ui_scale()) };
                }
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // Root stacking changes and damage on frames behind a live window backdrop carry no
            // window of ours; handle them before the window filter, which would put them back.
            if self.note_live_backdrop_event(&event) {
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            if unsafe { event.type_ } == MAPPING_NOTIFY {
                // Keymap changed (layout switch, setxkbmap, xmodmap). Refresh Xlib's cache so
                // later KeyPress decoding uses the new keysyms. Mapping events carry no window,
                // so they are handled before the window filter, which would put them back forever
                // in single-event mode.
                // SAFETY: the discriminator selects XMappingEvent; the copy is a valid event.
                let mut mapping = unsafe { event.xmapping };
                if mapping.request != MAPPING_POINTER {
                    // SAFETY: the display and event are live; Xlib only updates its keymap cache.
                    unsafe { XRefreshKeyboardMapping(&mut mapping) };
                }
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // SAFETY: xany is the common XEvent prefix and is valid for every event type.
            if self.single_event && unsafe { event.xany.window } != self.window {
                // Nothing else reads this display, so an event for another window is dropped.
                // Putting it back would hand the same event to the next pass forever: the loop
                // spins at full CPU and never reads input, paints or resizes again.
                note_foreign_event_dropped(&event);
                return Ok(());
            }
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            match unsafe { event.type_ } {
                EXPOSE => {
                    // The bound backbuffer already restored the exposed pixels when it holds the
                    // last frame at the current size. Otherwise a retained scene can predate a size
                    // or document change (a shrink), so the last Expose of a run relayouts and
                    // repaints. Coalesce counts.
                    if unsafe { event.xexpose.count } == 0
                        && !self.exposure_restored_by_background()
                    {
                        // SAFETY: The caller owns a live X11 display and app resources.
                        unsafe { self.repaint_full(document)? };
                    }
                }
                VISIBILITY_NOTIFY => {
                    // Every window selects VisibilityChangeMask (window_event_mask). Dispatched with the other
                    // window-level hand-offs below, so the host sees the latest verdict per turn.
                    // SAFETY: type_ selects the XVisibilityEvent variant of the union.
                    let state = unsafe { event.xvisibility.state };
                    self.pending_visibility = Some(state == VISIBILITY_FULLY_OBSCURED);
                }
                MAP_NOTIFY => {
                    // Native presenter: MapNotify advances Painted -> Mapped.
                    // Stray MapNotify in any other state is ignored, never an error.
                    let _ = self.note_mapped();
                }
                CONFIGURE_NOTIFY => {
                    // Any configure may be a move: the cached root origin is stale until re-queried.
                    self.root_origin = None;
                    // Reconcile-only: adopt the WM-driven size into retained
                    // state (recreate backbuffer, mark Full, shape follows).
                    // No XMoveResizeWindow echo: the native move is owned by
                    // SurfaceController::move_resize only.
                    // Coalesce: a drag queues many ConfigureNotify; only the newest size is adopted.
                    let window = self.window;
                    let display = self.display;
                    let configure = newest_configure(unsafe { event.xconfigure }, || {
                        if unsafe { XEventsQueued(display, QUEUED_AFTER_READING) } <= 0 {
                            return None;
                        }
                        let mut peeked = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XPeekEvent initializes the supplied XEvent storage.
                        unsafe { XPeekEvent(display, peeked.as_mut_ptr()) };
                        // SAFETY: XPeekEvent wrote a complete XEvent into the storage.
                        let peeked = unsafe { peeked.assume_init() };
                        // SAFETY: type_ and xany are the common XEvent prefix.
                        if unsafe { peeked.type_ } != CONFIGURE_NOTIFY
                            // SAFETY: xany is valid after checking the common prefix.
                            || unsafe { peeked.xany.window } != window
                        {
                            return None;
                        }
                        let mut queued = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XNextEvent removes the exact event XPeekEvent reported.
                        unsafe { XNextEvent(display, queued.as_mut_ptr()) };
                        // SAFETY: the removed event was checked as CONFIGURE_NOTIFY above.
                        Some(unsafe { queued.assume_init().xconfigure })
                    });
                    let width = configure.width.max(1) as u32;
                    let height = configure.height.max(1) as u32;
                    if self.window_backdrop.is_some() {
                        self.mark_window_backdrop_stale();
                        self.request_window_backdrop_refresh();
                    }
                    if width != self.width || height != self.height {
                        self.width = width;
                        self.height = height;
                        self.pending_window_resize = Some((width, height));
                        // SAFETY: The display, window, and retained backbuffer belong to this app.
                        unsafe { self.recreate_backbuffer()? };
                        // Shrink only once the resize has settled (no queued X events).
                        // SAFETY: display is live; the queue probe does not consume events.
                        if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                            unsafe { self.shrink_backbuffer_if_settled()? };
                        }
                        self.mark_full();
                        // SAFETY: window and display are live; mask follows resize.
                        unsafe { self.refresh_shape_mask(document) };
                    }
                }
                MOTION_NOTIFY => {
                    // Pointer motion can arrive much faster than a full scene redraw.
                    // Always consume the newest queued motion for this window before
                    // mutating layout/painting so interaction never trails behind the
                    // physical pointer by a backlog of stale coordinates.
                    // SAFETY: The discriminator identifies XMotionEvent in this match arm.
                    let mut motion = unsafe { event.xmotion };
                    loop {
                        // Only coalesce contiguous MotionNotify events. Do not
                        // search past ButtonRelease/other events, because doing
                        // so would violate X event ordering and could extend a
                        // drag beyond its release point. QUEUED_AFTER_READING also
                        // pulls already-arrived socket input into Xlib when the
                        // in-memory queue empties, preventing a nested X server
                        // from feeding us stale motion one packet at a time.
                        // SAFETY: self.display is the live display used by this event loop.
                        if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                            break;
                        }
                        let mut peeked = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XPeekEvent initializes the supplied XEvent storage.
                        unsafe { XPeekEvent(self.display, peeked.as_mut_ptr()) };
                        // SAFETY: XPeekEvent wrote a complete XEvent into the storage.
                        let peeked = unsafe { peeked.assume_init() };
                        // SAFETY: type_ and xany are the common XEvent prefix.
                        if unsafe { peeked.type_ } != MOTION_NOTIFY
                            // SAFETY: xany is valid for the event after checking its common prefix.
                            || unsafe { peeked.xany.window } != self.window
                        {
                            break;
                        }
                        let mut queued = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XNextEvent initializes the supplied XEvent storage.
                        unsafe { XNextEvent(self.display, queued.as_mut_ptr()) };
                        // SAFETY: The peeked event established a contiguous same-window motion
                        // event; XNextEvent removes that exact event from the queue.
                        let queued = unsafe { queued.assume_init() };
                        // SAFETY: The contiguous event was checked as MOTION_NOTIFY above.
                        motion = unsafe { queued.xmotion };
                    }
                    let hover = self.hit_test(document, motion.x as f32, motion.y as f32);
                    let changed = hover != self.interaction.hover;
                    self.interaction.hover = hover;
                    self.update_cursor(document, hover);
                    // Range drag: active node is a slider track; emit semantic
                    // delta via existing ActionEvent path (Motion, text=value).
                    if let Some((drag_index, track_x, track_w, min, max)) = self.range_drag {
                        let scale = document.ui_scale();
                        let text =
                            range_drag_text(track_x, track_w, min, max, motion.x as f32 / scale);
                        let mut event = action_event(
                            scale,
                            ActionPhase::Motion,
                            self.pointer_button.unwrap_or(1),
                            (motion.x, motion.y),
                            hover == Some(drag_index),
                            motion.time as u64,
                            Some(text),
                            motion.state,
                        );
                        if fire_node(on_action, document, &mut buffers, drag_index, &mut event)? {
                            self.mark_input_repaint();
                            if self.single_event && !self.input_backlog() {
                                unsafe { self.redraw_if_dirty(document)? };
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    if let (Some(button), Some(index)) =
                        (self.pointer_button, self.interaction.active)
                    {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Motion,
                            button,
                            (motion.x, motion.y),
                            hover == Some(index),
                            motion.time as u64,
                            None,
                            motion.state,
                        );
                        if fire_node(on_action, document, &mut buffers, index, &mut event)?
                            || changed
                        {
                            self.mark_input_repaint();
                        }
                    } else {
                        // Hover is a *pointer-position* event, not only an enter event.
                        // Radial controls can cross distinct polar sectors while hit-testing
                        // the same retained node. Dispatch on each motion and let semantic
                        // owners deduplicate unchanged state. Only changes of hovered node
                        // force a renderer redraw; actual document mutations carry damage.
                        if let Some(index) = hover {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Hover,
                                0,
                                (motion.x, motion.y),
                                true,
                                motion.time as u64,
                                None,
                                motion.state,
                            );
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        if changed {
                            self.mark_input_repaint();
                        }
                    }
                }
                LEAVE_NOTIFY => {
                    // SAFETY: The discriminator identifies XCrossingEvent in this match arm.
                    let crossing = unsafe { event.xcrossing };
                    let left = self.interaction.hover.take();
                    self.set_cursor(CursorKind::Default);
                    if let Some(index) = left {
                        // Leave is a hover with `inside == false` on the node that was hovered,
                        // so owners can reset their per-list highlight without a new action kind.
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Hover,
                            0,
                            (crossing.x, crossing.y),
                            false,
                            crossing.time as u64,
                            None,
                            crossing.state,
                        );
                        fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        self.mark_input_repaint();
                    }
                }
                KEY_PRESS => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&key) };
                    emit_typed_key(
                        &key,
                        typed_symbol,
                        ActionPhase::Press,
                        document,
                        on_action,
                        &mut buffers,
                    )?;
                    self.sync_keyboard_focus(document);
                    let ctrl = key.state & CONTROL_MASK != 0;
                    let alt = key.state & MOD1_MASK != 0;
                    let super_key = key.state & MOD4_MASK != 0;
                    let action = if alt && symbol == XK_TAB {
                        self.super_chord_used |= super_key;
                        Some("keyboard.alt-tab")
                    } else if ctrl && !alt && !super_key && symbol == XK_SPACE {
                        Some("keyboard.start.1")
                    } else if ctrl && super_key {
                        self.super_chord_used = true;
                        match symbol {
                            XK_LEFT => Some("keyboard.desktop.left.0"),
                            XK_RIGHT => Some("keyboard.desktop.right.0"),
                            XK_UP => Some("keyboard.desktop.up.0"),
                            XK_DOWN => Some("keyboard.desktop.down.0"),
                            _ => None,
                        }
                    } else if alt && !ctrl && !super_key {
                        match symbol {
                            XK_LEFT => Some("keyboard.desktop.left.1"),
                            XK_RIGHT => Some("keyboard.desktop.right.1"),
                            XK_UP => Some("keyboard.desktop.up.1"),
                            XK_DOWN => Some("keyboard.desktop.down.1"),
                            _ => None,
                        }
                    } else {
                        if super_key && symbol != XK_SUPER_L && symbol != XK_SUPER_R {
                            self.super_chord_used = true;
                        }
                        None
                    };
                    if let Some(action) = action {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            0,
                            (key.x, key.y),
                            true,
                            key.time as u64,
                            None,
                            key.state,
                        );
                        fire_static(on_action, document, &mut buffers, action, &mut event)?;
                        self.mark_input_repaint();
                    } else if inserts_text(key.state)
                        && let Some(text) = keyboard_text(typed_symbol)
                    {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            0,
                            (key.x, key.y),
                            true,
                            key.time as u64,
                            Some(text),
                            key.state,
                        );
                        fire_static(
                            on_action,
                            document,
                            &mut buffers,
                            "keyboard.input",
                            &mut event,
                        )?;
                        self.mark_input_repaint();
                    }
                }
                KEY_RELEASE => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&key) };
                    emit_typed_key(
                        &key,
                        typed_symbol,
                        ActionPhase::Release,
                        document,
                        on_action,
                        &mut buffers,
                    )?;
                    if symbol == XK_SUPER_L || symbol == XK_SUPER_R {
                        if !self.super_chord_used {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Release,
                                0,
                                (key.x, key.y),
                                true,
                                key.time as u64,
                                None,
                                key.state,
                            );
                            fire_static(
                                on_action,
                                document,
                                &mut buffers,
                                "keyboard.start.0",
                                &mut event,
                            )?;
                            self.mark_input_repaint();
                        }
                        self.super_chord_used = false;
                    }
                }
                BUTTON_PRESS => {
                    // SAFETY: The discriminator identifies XButtonEvent in this match arm.
                    let button = unsafe { event.xbutton };
                    let raw = pointer_button_from_raw(button.button);
                    if desktop_takes_focus_on_press(self.role, button.button) {
                        // SAFETY: The X11App owns the live display and mapped
                        // desktop window; focus changes only after a real
                        // primary button press, never on ordinary mouse motion.
                        unsafe {
                            crate::surface_controller::raw_press::XSetInputFocus(
                                self.display,
                                self.window,
                                crate::surface_controller::raw_press::REVERT_TO_POINTER_ROOT,
                                button.time,
                            );
                        }
                    }
                    // Wheel Button4/5: scroll delta via existing ActionEvent path
                    // (semantic "scroll" action, Motion phase), no press state.
                    if let Some((dx, dy)) = wheel_scroll_delta(raw) {
                        let scale = document.ui_scale();
                        if let Some(layout) = self.layout.as_ref() {
                            apply_wheel_scroll(
                                layout,
                                document,
                                self.interaction,
                                button.x as f32 / scale,
                                button.y as f32 / scale,
                                (dx, dy),
                            );
                        }
                        let mut event = action_event(
                            scale,
                            ActionPhase::Motion,
                            button.button,
                            (button.x, button.y),
                            true,
                            button.time as u64,
                            Some(format!("{dx},{dy}")),
                            button.state,
                        );
                        fire_static(on_action, document, &mut buffers, "scroll", &mut event)?;
                        // Same tick, same event shape, under the hit node's action name.
                        let hit = self.hit_test(document, button.x as f32, button.y as f32);
                        if let Some(index) = wheel_node_target(self.node_wheel_targets, hit) {
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        self.mark_input_repaint();
                        // A wheel burst shares one paint: defer while more input is queued.
                        if self.single_event && !self.input_backlog() {
                            unsafe { self.redraw_if_dirty(document)? };
                            return Ok(());
                        }
                        continue;
                    }
                    let hit = self.hit_test(document, button.x as f32, button.y as f32);
                    if button.button == 1 {
                        if let Some(index) = hit {
                            let now = button.time as u64;
                            let name: &str = &document.document.nodes[index as usize].action;
                            let is_title_move =
                                name.starts_with("window.") && name.ends_with(".move");
                            let double_title = is_title_move
                                && self
                                    .last_title_press
                                    .is_some_and(|(last_index, last_time)| {
                                        last_index == index && now.saturating_sub(last_time) <= 350
                                    });
                            if double_title {
                                let maximize = name
                                    .strip_prefix("window.")
                                    .and_then(|rest| rest.strip_suffix(".move"))
                                    .map(|prefix| format!("{prefix}.maximize"));
                                self.last_title_press = None;
                                self.interaction.active = None;
                                self.pointer_button = None;
                                if let Some(maximize) = maximize {
                                    let mut event = action_event(
                                        document.ui_scale(),
                                        ActionPhase::Release,
                                        button.button,
                                        (button.x, button.y),
                                        true,
                                        button.time as u64,
                                        None,
                                        button.state,
                                    );
                                    fire_static(
                                        on_action,
                                        document,
                                        &mut buffers,
                                        &maximize,
                                        &mut event,
                                    )?;
                                }
                                self.mark_input_repaint();
                                unsafe { self.redraw_if_dirty(document)? };
                                return Ok(());
                            }
                            if is_title_move {
                                self.last_title_press = Some((index, now));
                            } else {
                                self.last_title_press = None;
                            }
                        } else {
                            self.last_title_press = None;
                        }
                        self.interaction.active = hit;
                        self.pointer_button = Some(button.button);
                        if let Some(index) = hit {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Press,
                                button.button,
                                (button.x, button.y),
                                true,
                                button.time as u64,
                                None,
                                button.state,
                            );
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        // Range drag arm: action names starting with "range." or
                        // "slider." capture the press as a slider drag; pointer
                        // geometry comes from the retained layout box.
                        if let Some(index) = hit {
                            let press_action: &str =
                                &document.document.nodes[index as usize].action;
                            if (press_action.starts_with("range.")
                                || press_action.starts_with("slider."))
                                && let Some(rect) = self.node_global_rect(document, index)
                            {
                                self.range_drag = Some((
                                    index,
                                    rect.x / document.ui_scale(),
                                    rect.width / document.ui_scale(),
                                    0.0,
                                    100.0,
                                ));
                            }
                        }
                        self.mark_input_repaint();
                    }
                }
                BUTTON_RELEASE => {
                    // SAFETY: The discriminator identifies XButtonEvent in this match arm.
                    let button = unsafe { event.xbutton };
                    // End any active range drag; emit final value via Release.
                    if button.button == 1
                        && let Some((drag_index, track_x, track_w, min, max)) =
                            self.range_drag.take()
                    {
                        let text = range_drag_text(
                            track_x,
                            track_w,
                            min,
                            max,
                            button.x as f32 / document.ui_scale(),
                        );
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            button.button,
                            (button.x, button.y),
                            true,
                            button.time as u64,
                            Some(text),
                            button.state,
                        );
                        fire_node(on_action, document, &mut buffers, drag_index, &mut event)?;
                    }
                    let hit = self.hit_test(document, button.x as f32, button.y as f32);
                    let dispatch = if button.button == 1 {
                        self.pointer_button = None;
                        self.interaction.active.take()
                    } else if button.button == 2 || button.button == 3 {
                        hit
                    } else {
                        None
                    };
                    if let Some(index) = dispatch {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            button.button,
                            (button.x, button.y),
                            hit == Some(index),
                            button.time as u64,
                            None,
                            button.state,
                        );
                        fire_node(on_action, document, &mut buffers, index, &mut event)?;
                    }
                    self.mark_input_repaint();
                    let hover = self.hit_test(document, button.x as f32, button.y as f32);
                    self.interaction.hover = hover;
                    self.update_cursor(document, hover);
                }
                SELECTION_NOTIFY => {
                    // SAFETY: The discriminator identifies a SelectionNotify on this display.
                    unsafe { self.handle_xdnd_selection_notify(&event, document, on_action)? };
                }
                CLIENT_MESSAGE => {
                    // SAFETY: The discriminator identifies XClientMessageEvent in this arm.
                    let client = unsafe { event.xclient };
                    // SAFETY: The client message is a live XClientMessageEvent on this display.
                    let xdnd = unsafe { self.handle_xdnd_message(&client, document, on_action)? };
                    if !xdnd && self.wm_delete != 0
                        && client.format == 32
                        // SAFETY: XClientMessageEvent format 32 selects the initialized long data
                        // representation used by the WM_DELETE protocol.
                        && unsafe { client.data.l[0] } as u64 == self.wm_delete
                    {
                        self.close_requested = true;
                    }
                }
                _ => {}
            }
            // Coalesce: paint only once the X queue is drained, so a burst of input shares one frame.
            // Damage marks above merge (max coverage); Expose presented directly. The reactor turn
            // paints any damage left by an early return once the queue is empty.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                unsafe { self.redraw_if_dirty(document)? };
            }
            // Size reports coalesce: the latest settled size is delivered once the queue is empty.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                unsafe { self.flush_pending_window_resize(document, on_action)? };
            }
            if let Some(obscured) = self.pending_visibility.take() {
                let visibility = ActionEvent {
                    action: WINDOW_VISIBILITY_ACTION.to_string(),
                    phase: flamewm_render_core::ActionPhase::Release,
                    button: 0,
                    modifiers: 0,
                    x: if obscured { 1.0 } else { 0.0 },
                    y: 0.0,
                    inside: true,
                    time_ms: 0,
                    text: None,
                };
                on_action(&visibility, document)?;
            }
            if self.single_event {
                return Ok(());
            }
        }
    }
}
