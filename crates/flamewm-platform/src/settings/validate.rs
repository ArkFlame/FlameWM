use flamewm_api::FlameResult;

use super::{invalid, invalid_owned};
use crate::chrome::parse_hex_rgb;

pub(super) fn parse_bool(value: &str, name: &str) -> FlameResult<bool> {
    match value {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => invalid_owned(format!("bad {name}")),
    }
}
pub(super) fn is_valid_output_id(output: &str) -> bool {
    !output.is_empty()
        && !output
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '=')
}
pub(super) fn validate_color(value: &str, allow_empty: bool, name: &str) -> FlameResult<()> {
    if (allow_empty && value.is_empty()) || parse_hex_rgb(value).is_some() {
        return Ok(());
    }
    invalid_owned(format!(
        "{name} must be #RRGGBB{}",
        if allow_empty { " or empty" } else { "" }
    ))
}
/// Empty (built-in default) or an absolute path of at most 4096 bytes without control characters.
pub(super) fn validate_lock_image_path(value: &str, name: &str) -> FlameResult<()> {
    if value.len() > 4096
        || !(value.is_empty() || value.starts_with('/'))
        || value.chars().any(char::is_control)
    {
        return invalid_owned(format!(
            "{name} must be empty or an absolute path of at most 4096 bytes"
        ));
    }
    Ok(())
}
/// Empty, a bare command name, or an absolute path with no whitespace or control characters.
pub(super) fn validate_terminal_program(value: &str) -> FlameResult<()> {
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 255
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || value.contains('/') && !value.starts_with('/')
    {
        return invalid("defaultTerminal must be a command name or absolute path without spaces");
    }
    Ok(())
}
/// Desktop mode implied by saved values alone: a saved path means custom, otherwise bundled.
/// Old per-button style values: `square` is square, `default|flat|circle` are circle.
pub(super) fn legacy_button_style_is_square(value: &str) -> FlameResult<bool> {
    match value {
        "square" => Ok(true),
        "default" | "flat" | "circle" => Ok(false),
        _ => invalid("titlebar button style must be default/flat/circle/square"),
    }
}
