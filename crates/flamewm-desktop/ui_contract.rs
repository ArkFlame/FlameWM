//! Shell-critical desktop UI IDs shared by the build script and runtime.
//!
//! The compiled RWR, not the authored HTML, must contain these elements.

pub const REQUIRED_DESKTOP_IDS: &[&str] = &[
    "desktop",
    "desktop-wallpaper-selection",
    "desktop-watermark",
    "desktop-selection",
    "desktop-drag-ghost",
    "desktop-convert-preview",
    "desktop-convert-preview-icon",
    "desktop-convert-preview-line-1",
    "desktop-convert-preview-line-2",
    "desktop-convert-preview-status",
    "desktop-rename",
    "desktop-menu",
    "desktop-entry-menu",
    "desktop-convert-wheel",
    "menu-row-convert",
    "sticky-menu",
    "desktop-confirm",
    "convert-wheel-hub-text",
    "convert-wheel-highlight",
    "convert-slot-0",
    "convert-slot-1",
    "convert-slot-2",
    "convert-slot-3",
    "convert-slot-4",
    "convert-slot-5",
    "convert-slot-6",
    "convert-slot-7",

];

/// Reject an old/partial compiled document before it reaches desktop projection.
/// The caller owns the lookup, so build and runtime check the same inventory.
pub fn require_desktop_nodes(mut contains: impl FnMut(&str) -> bool) -> Result<(), String> {
    for id in REQUIRED_DESKTOP_IDS {
        if !contains(id) {
            return Err(format!(
                "desktop compiled RWR is missing required node '{id}' (stale desktop UI artifact or incomplete UI merge)"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_contract_passes() {
        assert!(require_desktop_nodes(|id| REQUIRED_DESKTOP_IDS.contains(&id)).is_ok());
    }

    #[test]
    fn missing_convert_wheel_fails_the_original_startup_gate() {
        let error = require_desktop_nodes(|id| {
            id != "desktop-convert-wheel" && REQUIRED_DESKTOP_IDS.contains(&id)
        })
        .expect_err("old compiled UI must not be allowed to launch");
        assert!(error.contains("desktop-convert-wheel"), "{error}");
    }

    #[test]
    fn missing_one_conversion_choice_cannot_pass_as_complete() {
        let error = require_desktop_nodes(|id| {
            id != "convert-slot-7" && REQUIRED_DESKTOP_IDS.contains(&id)
        })
        .expect_err("an incomplete radial menu is not valid");
        assert!(error.contains("convert-slot-7"), "{error}");
    }
}
