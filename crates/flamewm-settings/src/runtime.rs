use flamewm_ui_x11::{UiControllerEvent, UiWindowRole, run_with_controller_events_role};

use crate::SettingsApplication;
use crate::control::SettingsControl;
use crate::view;

#[path = "launch.rs"]
pub mod launch;
pub use launch::SettingsLaunchOptions;

pub fn run(options: SettingsLaunchOptions) -> Result<(), String> {
    let mut application = SettingsApplication::new(
        SettingsControl::connect_session().map_err(|error| error.to_string())?,
    );
    application.client_mut().set_page(options.page);
    let mut document = view::document()?;
    application.refresh(&mut document)?;
    run_with_controller_events_role(
        document,
        super::normal_window_config(view::SETTINGS_DEFAULT_WIDTH, view::SETTINGS_DEFAULT_HEIGHT),
        UiWindowRole::Normal,
        move |event, document| {
            application.poll_background(document)?;
            match event {
                UiControllerEvent::Action(action) => application.handle_action(action, document)?,
                UiControllerEvent::Key(key) => application.handle_key_event(key, document)?,
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn launch_page_is_selected_before_first_refresh() {
        let production = include_str!("runtime.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("runtime production source");
        let page_selection = production
            .find("set_page")
            .expect("runtime must apply the requested page");
        let refresh = production
            .find("application.refresh")
            .expect("runtime must refresh the application");
        assert!(
            page_selection < refresh,
            "requested page must be selected before refresh to avoid an Appearance first frame"
        );
    }

    #[test]
    fn navigation_projection_moves_the_compiled_selection_marker() {
        let application = include_str!("lib.rs");
        assert!(
            application.contains("nav-selection"),
            "settings synchronization must own the compiled navigation marker"
        );
        assert!(
            application.contains("position("),
            "settings synchronization must move the navigation marker"
        );
    }
}
