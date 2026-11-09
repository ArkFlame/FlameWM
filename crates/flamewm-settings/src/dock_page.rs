//! Dependent rows of the Dock page. Pure: the page projects these flags onto its markup.
//!
//! The "Show dock" toggle always shows. Every other dock option needs the dock on; "Reserve space"
//! also needs auto-hide off (an auto-hidden dock never reserves); the custom surface controls need
//! the custom surface switch on.

/// Visibility of the dependent Dock rows for one switch state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockRows {
    /// Every option below the "Show dock" toggle.
    pub(crate) options: bool,
    /// The "Reserve space" toggle.
    pub(crate) reserve_space: bool,
    /// The custom surface colour, blur, opacity and outline rows.
    pub(crate) surface_controls: bool,
}

#[must_use]
pub(crate) fn dock_rows(enabled: bool, auto_hide: bool, surface_custom: bool) -> DockRows {
    DockRows {
        options: enabled,
        reserve_space: enabled && !auto_hide,
        surface_controls: enabled && surface_custom,
    }
}

#[cfg(test)]
mod tests {
    use super::dock_rows;

    #[test]
    fn disabled_dock_shows_no_other_option() {
        for auto_hide in [false, true] {
            for custom in [false, true] {
                let rows = dock_rows(false, auto_hide, custom);
                assert!(!rows.options);
                assert!(!rows.reserve_space);
                assert!(!rows.surface_controls);
            }
        }
    }

    #[test]
    fn reserve_space_is_hidden_while_auto_hide_is_on() {
        assert!(dock_rows(true, false, false).reserve_space);
        assert!(!dock_rows(true, true, false).reserve_space);
        assert!(!dock_rows(true, true, true).reserve_space);
    }

    #[test]
    fn surface_controls_need_the_dock_and_the_custom_switch() {
        assert!(!dock_rows(true, false, false).surface_controls);
        assert!(dock_rows(true, false, true).surface_controls);
        assert!(!dock_rows(false, false, true).surface_controls);
    }

    #[test]
    fn enabled_dock_always_shows_its_options() {
        assert!(dock_rows(true, true, false).options);
        assert!(dock_rows(true, false, true).options);
    }
}
