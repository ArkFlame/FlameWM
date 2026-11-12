//! Bluetooth pairing card model: the prompt or notice on screen, the typed PIN or passkey, and the
//! error line. The agent owns the queue; this model holds only what the card has typed.

use flamewm_integrations_linux::bluetooth_agent::{check_typed, Decision, PromptKind};

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
}

impl BluetoothDialog {
    /// Keeps the typed text with its prompt: any other prompt id, or a notice (`None`), clears it.
    pub fn follow(&mut self, id: Option<u64>) {
        if id != self.active {
            self.active = id;
            self.typed.clear();
            self.error = None;
        }
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
