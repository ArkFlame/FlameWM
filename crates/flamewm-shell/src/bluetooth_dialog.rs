//! Bluetooth pairing card model: the prompt or notice on screen, the typed PIN or passkey, and the
//! error line. The agent owns the queue; this model holds only what the card has typed.

//! Keyboard model: while the typed field has focus, printable keys and Space edit the value. Tab
//! leaves the field into the two buttons and walks them (Shift+Tab backward, wrapping). Space on a
//! focused button activates it once; Enter always accepts. Precedent: WAI-ARIA APG dialog (Tab
//! moves between controls, Space activates a button) and KDE bluedevil's pairing wizard, whose
//! PIN page hands focus to its buttons. KDE is not vendored under `.vendor/`; idea only.

use flamewm_integrations_linux::bluetooth_agent::{check_typed, Decision, PromptKind};

use crate::focus_keys::tab_focus_after;

/// Focus stops on the card, in Tab order: Reject, then Accept.
pub const BUTTON_IDS: [&str; 2] = ["bluetooth-reject", "bluetooth-accept"];
/// Action dispatched by each button, indexed like `BUTTON_IDS`.
pub const BUTTON_ACTIONS: [&str; 2] = ["bluetooth.reject", "bluetooth.accept"];

/// Focus after a typed Tab on the card; `None` for any other key.
#[must_use]
pub fn tab_focus_after_key(payload: &str, focus: Option<usize>) -> Option<usize> {
    tab_focus_after(payload, focus, BUTTON_IDS.len())
}

/// Action of the focused button when a typed Space activates it; `None` otherwise. Enter is not
/// listed: it accepts the prompt on every focus.
#[must_use]
pub fn button_activation(text: Option<&str>, focus: Option<usize>) -> Option<&'static str> {
    let index = focus.filter(|index| *index < BUTTON_ACTIONS.len())?;
    matches!(text, Some(" ")).then_some(BUTTON_ACTIONS[index])
}

/// Focus ring per button: true on the focused button, false elsewhere.
#[must_use]
pub fn button_focus_ring(focus: Option<usize>) -> [bool; BUTTON_IDS.len()] {
    std::array::from_fn(|index| focus == Some(index))
}

/// Longest typed value. A PIN is at most 16 characters; a passkey is at most six digits.
pub const MAX_TYPED_CHARS: usize = 16;

/// What the card shows: the oldest decision request, else the display notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Prompt { id: u64, kind: PromptKind },
    Notice(PromptKind),
}

#[derive(Default)]
pub struct BluetoothDialog {
    /// Prompt the typed text belongs to. A different prompt or a notice clears the text.
    active: Option<u64>,
    typed: String,
    error: Option<&'static str>,
    /// Focused button index (see `BUTTON_IDS`); `None` keeps focus on the typed field.
    focus: Option<usize>,
}

impl BluetoothDialog {
    /// Keeps the typed text with its prompt: any other prompt id, or a notice (`None`), clears it
    /// and returns focus to the field.
    pub fn follow(&mut self, id: Option<u64>) {
        if id != self.active {
            self.active = id;
            self.typed.clear();
            self.error = None;
            self.focus = None;
        }
    }

    #[must_use]
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    pub fn set_focus(&mut self, focus: Option<usize>) {
        self.focus = focus;
    }

    #[must_use]
    pub fn typed(&self) -> &str {
        &self.typed
    }

    #[must_use]
    pub fn error(&self) -> Option<&'static str> {
        self.error
    }

    /// Appends printable text, up to `MAX_TYPED_CHARS`.
    pub fn input(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            if self.typed.chars().count() >= MAX_TYPED_CHARS {
                break;
            }
            self.typed.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        self.typed.pop();
    }

    /// The decision for an accept press. An invalid typed value keeps the prompt open and sets the
    /// reason on the card.
    pub fn accept(&mut self, kind: &PromptKind) -> Option<Decision> {
        let typed = match kind {
            PromptKind::PinCode { .. } | PromptKind::Passkey { .. } => Some(self.typed.clone()),
            _ => None,
        };
        if let Some(text) = typed.as_deref() {
            if let Err(reason) = check_typed(kind, text) {
                self.error = Some(reason);
                return None;
            }
        }
        Some(Decision::Accept(typed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

    fn pin() -> PromptKind {
        PromptKind::PinCode {
            device: DEVICE.to_owned(),
        }
    }

    #[test]
    fn a_new_prompt_clears_the_typed_value_and_error() {
        let mut dialog = BluetoothDialog::default();
        dialog.follow(Some(1));
        dialog.input("12");
        assert_eq!(
            dialog.accept(&pin()),
            Some(Decision::Accept(Some("12".to_owned())))
        );
        dialog.follow(Some(2));
        assert_eq!(dialog.typed(), "");
        assert_eq!(dialog.error(), None);
    }

    #[test]
    fn typing_is_capped_and_backspace_edits() {
        let mut dialog = BluetoothDialog::default();
        dialog.follow(Some(1));
        dialog.input(&"9".repeat(40));
        assert_eq!(dialog.typed().len(), MAX_TYPED_CHARS);
        dialog.backspace();
        assert_eq!(dialog.typed().len(), MAX_TYPED_CHARS - 1);
        dialog.input("\u{8}\n");
        assert_eq!(dialog.typed().len(), MAX_TYPED_CHARS - 1);
    }

    #[test]
    fn invalid_passkey_keeps_the_prompt_open_with_its_reason() {
        let mut dialog = BluetoothDialog::default();
        dialog.follow(Some(1));
        dialog.input("1000000");
        let passkey = PromptKind::Passkey {
            device: DEVICE.to_owned(),
        };
        assert_eq!(dialog.accept(&passkey), None);
        assert_eq!(dialog.error(), Some("Passkey is a number up to 999999"));
        dialog.backspace();
        assert_eq!(
            dialog.accept(&passkey),
            Some(Decision::Accept(Some("100000".to_owned())))
        );
    }

    #[test]
    fn confirmation_accept_carries_no_value() {
        let mut dialog = BluetoothDialog::default();
        dialog.follow(Some(1));
        let confirm = PromptKind::Confirm {
            device: DEVICE.to_owned(),
            passkey: 123_456,
        };
        assert_eq!(dialog.accept(&confirm), Some(Decision::Accept(None)));
    }
}
