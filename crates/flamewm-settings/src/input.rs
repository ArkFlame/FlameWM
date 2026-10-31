//! Split from `lib.rs` (`input`); see crate docs.

use super::*;

impl<T: ControlTransport> Drop for SettingsApplication<T> {
    fn drop(&mut self) {
        // Best effort; loss of the owning D-Bus connection also restores backend grabs.
        let _ = self.client.cancel_hotkey_capture();
    }
}

/// Writes to a node the compiled markup may not declare yet. Best effort, so a
/// document without the optional node keeps projecting its required nodes.
pub(crate) fn optional_text(document: &mut impl UiDocumentAccess, id: &str, text: String) {
    let _ = document.text(id, text);
}

pub(crate) fn optional_visible(document: &mut impl UiDocumentAccess, id: &str, visible: bool) {
    let _ = document.visible(id, visible);
}

/// Bare Super commits as `Super` when its release carries no other modifier.
/// The release reports its own Super bit, so only that bit may be present.
pub(crate) fn bare_super_binding(event: &UiKeyEvent) -> Option<&'static str> {
    let is_super = matches!(
        event.key.to_ascii_lowercase().as_str(),
        "super" | "super_l" | "super_r"
    );
    let only_super = event
        .modifiers
        .iter()
        .all(|modifier| modifier.eq_ignore_ascii_case("super"));
    (is_super && only_super).then_some("Super")
}

pub(crate) fn is_modifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "ctrl"
            | "control"
            | "control_l"
            | "control_r"
            | "shift"
            | "shift_l"
            | "shift_r"
            | "alt"
            | "alt_l"
            | "alt_r"
            | "super"
            | "super_l"
            | "super_r"
            | "meta"
            | "meta_l"
            | "meta_r"
    )
}
