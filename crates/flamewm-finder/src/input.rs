//! Keyboard and button handling for the finder window: keys, actions and overlays.

use super::*;

impl Finder {
    pub(super) fn key(
        &mut self,
        key: &UiKeyEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press {
            return Ok(());
        }
        let has = |name: &str| key.modifiers.iter().any(|m| m.eq_ignore_ascii_case(name));
        let mods = Modifiers {
            ctrl: has("ctrl") || has("control"),
            shift: has("shift"),
            alt: has("alt"),
        };
        let Some(command) = keys::command(&key.key, key.text.as_deref(), mods, self.navigating)
        else {
            return Ok(());
        };
        let allowed = match self.overlay {
            Overlay::None => true,
            Overlay::Preview | Overlay::Details => matches!(
                command,
                Command::Escape
                    | Command::Open
                    | Command::Reveal
                    | Command::Preview
                    | Command::Details
                    | Command::Help
            ),
            Overlay::Verbs => matches!(
                command,
                Command::Escape | Command::Actions | Command::Up | Command::Down | Command::Open
            ),
            Overlay::Confirm => matches!(
                command,
                Command::Escape | Command::Up | Command::Down | Command::Open
            ),
            Overlay::Help | Overlay::Duplicates | Overlay::Setup => {
                matches!(command, Command::Escape | Command::Help)
            }
        };
        if !allowed {
            return Ok(());
        }
        match command {
            Command::Escape => match self.overlay {
                Overlay::None => self.quit(),
                _ => self.overlay = Overlay::None,
            },
            Command::Help => self.toggle(Overlay::Help),
            Command::Open if self.sheet_open() => self.run_verb(self.verb),
            Command::Open => self.activate_selected(),
            Command::Reveal => self.open_selected(true),
            Command::Preview => self.toggle(Overlay::Preview),
            Command::Details => self.toggle(Overlay::Details),
            Command::Actions => self.toggle_verbs(),
            Command::Up if self.sheet_open() => self.move_verb(-1),
            Command::Down if self.sheet_open() => self.move_verb(1),
            Command::Previous => {
                self.navigating = true;
                self.step(-1);
            }
            Command::Up => {
                self.navigating = true;
                self.step(-self.vertical_step());
            }
            Command::Down => {
                self.navigating = true;
                self.step(self.vertical_step());
            }
            Command::Left if self.tiles_shown() => {
                self.navigating = true;
                self.step(-1);
            }
            Command::Right if self.tiles_shown() => {
                self.navigating = true;
                self.step(1);
            }
            Command::Left | Command::Right => {}
            Command::ToggleView => self.grid = !self.grid,
            Command::PageDown => {
                self.navigating = true;
                self.step(isize::try_from(ROW_SLOTS).unwrap_or(8));
            }
            Command::PageUp => {
                self.navigating = true;
                self.step(-isize::try_from(ROW_SLOTS).unwrap_or(8));
            }
            Command::First => {
                self.navigating = true;
                self.select(0);
            }
            Command::Last => {
                self.navigating = true;
                self.select(usize::MAX);
            }
            Command::Backspace => {
                self.navigating = false;
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query);
            }
            Command::Insert(text) => {
                self.navigating = false;
                let mut query = self.query.clone();
                query.extend(text.chars().filter(|c| !c.is_control()));
                if query.chars().count() > QUERY_LIMIT {
                    query = query.chars().take(QUERY_LIMIT).collect();
                }
                self.set_query(query);
            }
        }
        self.paint(doc)
    }

    pub(super) fn action(
        &mut self,
        event: &UiActionEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.phase != UiActionPhase::Press {
            return Ok(());
        }
        match event.action.as_str() {
            "finder.clear" => {
                self.navigating = false;
                self.set_query(String::new());
            }
            "finder.close-overlay" => self.overlay = Overlay::None,
            action if action.starts_with("finder.verb.") => {
                if let Some(slot) = action
                    .strip_prefix("finder.verb.")
                    .and_then(|slot| slot.parse().ok())
                {
                    self.run_verb(slot);
                }
            }
            action => {
                let slot = action
                    .strip_prefix("finder.row.")
                    .or_else(|| action.strip_prefix("finder.tile."))
                    .and_then(|slot| slot.parse::<usize>().ok());
                if let Some(slot) = slot {
                    let position = self.first + slot;
                    if position < self.items().len() {
                        self.navigating = true;
                        let repeat = self.last_click.is_some_and(|(prior, at)| {
                            prior == position && at.elapsed() < DOUBLE_CLICK
                        });
                        self.select(position);
                        self.last_click = Some((position, Instant::now()));
                        if repeat {
                            self.activate_selected();
                        }
                    }
                }
            }
        }
        self.paint(doc)
    }
}
