use super::*;
use crate::bluetooth_dialog::{button_focus_ring, BluetoothDialog, Shown, BUTTON_IDS};
use flamewm_integrations_linux::bluetooth_agent::PromptKind;

/// Projects the card into `ui/shell/bluetooth.html`: the request text and its details, the entry
/// label, the typed value as asterisks (never the text), the error line and the two button labels.
/// Notices reuse the same card with OK and Dismiss.
pub fn project_bluetooth(
    document: &mut impl UiDocumentAccess,
    shown: &Shown,
    dialog: &BluetoothDialog,
) -> Result<(), String> {
    let kind = match shown {
        Shown::Prompt { kind, .. } | Shown::Notice(kind) => kind,
    };
    let card = describe(kind);
    document.text("bluetooth-message", card.message)?;
    document.text("bluetooth-details", card.details)?;
    document.text("bluetooth-label", card.entry.to_owned())?;
    document.text(
        "bluetooth-entry",
        "*".repeat(dialog.typed().chars().count()),
    )?;
    document.text(
        "bluetooth-error",
        dialog.error().unwrap_or_default().to_owned(),
    )?;
    document.text("bluetooth-accept", card.accept.to_owned())?;
    document.text("bluetooth-reject", card.reject.to_owned())?;
    for (id, ringed) in BUTTON_IDS.iter().zip(button_focus_ring(dialog.focus())) {
        if ringed {
            document.border(id, crate::projection::task_accent())?;
        } else {
            document.border_clear(id)?;
        }
    }
    Ok(())
}

/// Copy for one kind. `entry` is the label of the typed field, empty when nothing is typed.
struct Card {
    message: String,
    details: String,
    entry: &'static str,
    accept: &'static str,
    reject: &'static str,
}

/// Button and text choices follow the `BlueZ` agent API: confirmations and authorizations get
/// Accept/Reject, PIN and passkey requests take typed input, display notices show only.
/// Precedent: `.vendor/cosmic-epoch` bluetooth applet pairing dialogs and KDE bluedevil's pairing
/// wizard (confirm passkey, enter PIN, authorize service).
fn describe(kind: &PromptKind) -> Card {
    match kind {
        PromptKind::Confirm { device, passkey } => Card {
            message: "Confirm that this passkey matches the one on the device.".to_owned(),
            details: format!("Device: {device}\nPasskey: {passkey:06}"),
            entry: "",
            accept: "Confirm",
            reject: "Reject",
        },
        PromptKind::Authorize { device } => Card {
            message: "Allow this device to pair?".to_owned(),
            details: format!("Device: {device}"),
            entry: "",
            accept: "Allow",
            reject: "Reject",
        },
        PromptKind::AuthorizeService { device, uuid } => Card {
            message: "Allow this device to use a service?".to_owned(),
            details: format!("Device: {device}\nService: {uuid}"),
            entry: "",
            accept: "Allow",
            reject: "Reject",
        },
        PromptKind::PinCode { device } => Card {
            message: "Type the PIN for this device.".to_owned(),
            details: format!("Device: {device}"),
            entry: "PIN",
            accept: "Pair",
            reject: "Reject",
        },
        PromptKind::Passkey { device } => Card {
            message: "Type the passkey for this device.".to_owned(),
            details: format!("Device: {device}"),
            entry: "Passkey",
            accept: "Pair",
            reject: "Reject",
        },
        PromptKind::DisplayPasskey {
            device,
            passkey,
            entered,
        } => Card {
            message: "Type this passkey on the device.".to_owned(),
            details: format!("Device: {device}\nPasskey: {passkey:06}\nTyped: {entered} digits"),
            entry: "",
            accept: "OK",
            reject: "Dismiss",
        },
        PromptKind::DisplayPinCode { device, pin } => Card {
            message: "Type this PIN on the device.".to_owned(),
            details: format!("Device: {device}\nPIN: {pin}"),
            entry: "",
            accept: "OK",
            reject: "Dismiss",
        },
    }
}

// OBSERVABLE: the focused button is ringed and only it. MUTATION: ringing the wrong index fails.
#[cfg(test)]
mod focus_tests {
    use crate::bluetooth_dialog::{
        button_activation, button_focus_ring, tab_focus_after_key, BUTTON_IDS,
    };

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    #[test]
    fn tab_from_field_enters_buttons_and_wraps() {
        assert_eq!(tab_focus_after_key(&key("", "Tab"), None), Some(0));
        assert_eq!(tab_focus_after_key(&key("", "Tab"), Some(0)), Some(1));
        assert_eq!(tab_focus_after_key(&key("", "Tab"), Some(1)), Some(0));
        assert_eq!(tab_focus_after_key(&key("Shift", "Tab"), None), Some(1));
        assert_eq!(tab_focus_after_key(&key("", "Down"), None), None);
    }

    #[test]
    fn space_activates_only_the_focused_button() {
        assert_eq!(
            button_activation(Some(" "), Some(0)),
            Some("bluetooth.reject")
        );
        assert_eq!(
            button_activation(Some(" "), Some(1)),
            Some("bluetooth.accept")
        );
        assert_eq!(button_activation(Some(" "), None), None);
        assert_eq!(button_activation(Some("1"), Some(1)), None);
        assert_eq!(button_activation(Some("\n"), Some(1)), None);
    }

    #[test]
    fn focus_ring_marks_only_the_focused_button() {
        assert_eq!(button_focus_ring(None), [false; 2]);
        assert_eq!(button_focus_ring(Some(1)), [false, true]);
    }

    // REGRESSION: button ids exist in bluetooth.html and the button rule keeps a 1px border box.
    #[test]
    fn bluetooth_focus_ids_exist_in_html_with_a_ring_border_box() {
        let html = include_str!("../../../../ui/shell/bluetooth.html");
        for id in BUTTON_IDS {
            assert!(html.contains(&format!("id=\"{id}\"")), "{id} missing");
        }
        assert!(html.contains(
            ".bluetooth-button { height:28px; padding:0px var(--space-xl); border:1px solid"
        ));
    }
}
