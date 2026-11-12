use super::*;
use crate::polkit_dialog::DialogModel;

/// Projects the active prompt into `ui/shell/polkit.html`: message, detail rows, the identity
/// row, a masked password (asterisks per byte, never the text) and the error line.
///
/// The identity `<select>` is label-only: `UiDocumentAccess` has no option or child API, so the
/// choice list cannot be populated. Single-identity prompts (the common case) read correctly.
/// shortcut: multi-identity select options; revisit when `UiDocumentAccess` gains option support.
pub fn project_polkit(
    document: &mut impl UiDocumentAccess,
    model: &DialogModel,
) -> Result<(), String> {
    document.text("polkit-message", model.message.clone())?;
    let details = model
        .details
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    document.text("polkit-details", details)?;
    let identity = model
        .identities
        .get(model.selected)
        .map_or_else(String::new, |choice| choice.label.clone());
    document.text("polkit-identity", identity)?;
    document.text("polkit-password", "*".repeat(model.password.len()))?;
    document.text("polkit-error", model.error.clone().unwrap_or_default())
}
