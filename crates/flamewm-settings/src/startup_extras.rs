//! Startup Applications additions: the installed-app picker and the custom command field.
//! Kept apart from `app_impl_3` so the page projection stays within its size budget.

use super::*;
use crate::startup_page;

/// Longest custom command the keyboard can type.
const COMMAND_MAX_CHARS: usize = 512;

/// Typed custom command not yet added, with the inline message for the last Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupCommandDraft {
    pub(crate) buffer: String,
    pub(crate) error: Option<String>,
}

/// Feeds typed text into the command draft. Enter submits the buffer and closes the field on
/// success, keeping it open with the message on failure; Escape discards it.
pub(crate) fn type_command(
    draft: &mut Option<StartupCommandDraft>,
    text: &str,
    submit: impl Fn(&str) -> Result<(), String>,
) {
    for ch in text.chars() {
        match ch {
            '\u{1b}' => {
                *draft = None;
                return;
            }
            '\n' | '\r' => {
                let Some(current) = draft.as_mut() else {
                    return;
                };
                match submit(&current.buffer) {
                    Ok(()) => {
                        *draft = None;
                        return;
                    }
                    Err(error) => current.error = Some(error),
                }
            }
            '\u{8}' => {
                if let Some(current) = draft.as_mut() {
                    current.buffer.pop();
                    current.error = None;
                }
            }
            ch if !ch.is_control() => {
                if let Some(current) = draft.as_mut()
                    && current.buffer.chars().count() < COMMAND_MAX_CHARS
                {
                    current.buffer.push(ch);
                    current.error = None;
                }
            }
            _ => {}
        }
    }
}

impl<T: ControlTransport> SettingsApplication<T> {
    /// Feeds typed text into the command draft, submitting through the startup-page facade.
    pub(crate) fn startup_command_text(&mut self, text: &str) {
        type_command(&mut self.startup_command, text, startup_page::add_command);
    }

    /// Projects the picker's current candidate and the command field.
    pub(crate) fn sync_startup_extras(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let candidates = startup_page::candidates();
        let (name, can_add) = if candidates.is_empty() {
            (
                "Every installed application already starts at sign-in".to_owned(),
                false,
            )
        } else {
            (
                candidates[self.startup_pick % candidates.len()]
                    .name
                    .clone(),
                true,
            )
        };
        document.text("startup-pick-name", name)?;
        document.visible("startup-add", can_add)?;

        let (text, error) = self
            .startup_command
            .as_ref()
            .map_or((String::new(), None), |draft| {
                (draft.buffer.clone(), draft.error.clone())
            });
        let shown = if text.is_empty() {
            "Add a custom command".to_owned()
        } else {
            text
        };
        document.text("startup-command-text", shown)?;
        document.text("startup-command-error", error.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(_: &str) -> Result<(), String> {
        Ok(())
    }

    #[test]
    fn typing_edits_the_buffer_and_escape_discards_it() {
        let mut draft = Some(StartupCommandDraft {
            buffer: String::new(),
            error: None,
        });
        type_command(&mut draft, "ab", accept);
        type_command(&mut draft, "\u{8}c", accept);
        assert_eq!(draft.as_ref().unwrap().buffer, "ac");
        type_command(&mut draft, "\u{1b}", accept);
        assert!(draft.is_none());
    }

    #[test]
    fn enter_keeps_the_draft_open_with_the_error_then_closes_on_success() {
        let mut draft = Some(StartupCommandDraft {
            buffer: "!!!".into(),
            error: None,
        });
        type_command(&mut draft, "\n", |command| {
            Err(format!("rejected {command}"))
        });
        let open = draft.clone().expect("failed submit keeps the draft");
        assert_eq!(open.error.as_deref(), Some("rejected !!!"));

        type_command(&mut draft, "x\n", accept);
        assert!(draft.is_none(), "successful submit closes the field");
    }

    #[test]
    fn typing_is_capped_and_ignored_without_a_draft() {
        let mut draft = None;
        type_command(&mut draft, "abc", accept);
        assert!(draft.is_none());

        let mut draft = Some(StartupCommandDraft {
            buffer: "x".repeat(COMMAND_MAX_CHARS),
            error: None,
        });
        type_command(&mut draft, "y", accept);
        assert_eq!(draft.unwrap().buffer.chars().count(), COMMAND_MAX_CHARS);
    }
}
