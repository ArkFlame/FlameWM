use std::cell::RefCell;
use std::rc::Rc;

use flamewm_settings_core::ControlTransport;
use flamewm_ui_x11::{
    UiControllerEvent, UiDocumentAccess, UiWakeDrain, UiWindowRole,
    run_with_controller_events_role_and_two_readable_fds,
};

use crate::SettingsApplication;
use crate::control::{
    KnownRevisions, SettingsControl, SettingsScope, SettingsSignals, SettingsWake, reproject_scope,
};
use crate::file_chooser::WallpaperChooserWake;
use crate::view;

#[path = "launch.rs"]
pub mod launch;
pub use launch::SettingsLaunchOptions;

pub fn run(options: SettingsLaunchOptions) -> Result<(), String> {
    let mut application = SettingsApplication::new(
        SettingsControl::connect_session().map_err(|error| error.to_string())?,
    );
    application.client_mut().set_page(options.page);
    application.appearance_tab = options.appearance_tab;
    let mut document = view::document()?;
    if let Err(message) = application.refresh(&mut document) {
        application.report_error(&mut document, message);
    }

    // The bus signal connection is a reactor source. Its drain only queues items; the window loop
    // applies them in `on_wake` with document access. Nothing refreshes without a bus wake.
    let signals = SettingsSignals::connect_session().map_err(|error| error.to_string())?;
    let signal_fd = signals.raw_fd();
    let signals = Rc::new(signals);
    let mut wallpaper_wake = WallpaperChooserWake::new()?;
    let wallpaper_fd = wallpaper_wake.raw_fd();
    let wakes = Rc::new(RefCell::new(Vec::<SettingsWake>::new()));
    let drain = {
        let signals = Rc::clone(&signals);
        let wakes = Rc::clone(&wakes);
        move || match signals.drain(|signal| wakes.borrow_mut().push(SettingsWake::Signal(signal)))
        {
            Ok(_) => UiWakeDrain::Keep,
            Err(error) => {
                wakes
                    .borrow_mut()
                    .push(SettingsWake::Lost(error.to_string()));
                UiWakeDrain::Retire
            }
        }
    };

    // Started after the completion wake is registered, so a fast scan result is never missed.
    application.start_theme_scan();
    let application = Rc::new(RefCell::new(application));
    let event_application = Rc::clone(&application);
    let wake_application = Rc::clone(&application);
    let wake_queue = Rc::clone(&wakes);
    run_with_controller_events_role_and_two_readable_fds(
        document,
        {
            let (width, height) = options
                .window_size
                .unwrap_or((view::SETTINGS_DEFAULT_WIDTH, view::SETTINGS_DEFAULT_HEIGHT));
            super::normal_window_config(width, height)
        },
        UiWindowRole::Normal,
        signal_fd,
        drain,
        Some((
            wallpaper_fd,
            Box::new(move || {
                wallpaper_wake.drain();
                UiWakeDrain::Keep
            }),
        )),
        move |event, document| {
            let mut application = event_application.borrow_mut();
            // Request failures are shown inline and never end the window loop.
            if let Err(message) = application.poll_background(document) {
                application.report_error(document, message);
            }
            let result = match event {
                UiControllerEvent::Action(action) => application.handle_action(action, document),
                UiControllerEvent::Key(key) => application.handle_key_event(key, document),
                UiControllerEvent::Resized { .. } => Ok(()),
            };
            if let Err(message) = result {
                application.report_error(document, message);
            }
            Ok(())
        },
        move |document| {
            {
                let mut application = wake_application.borrow_mut();
                if let Err(message) = application.poll_background(document) {
                    application.report_error(document, message);
                }
            }
            let pending = std::mem::take(&mut *wake_queue.borrow_mut());
            if pending.is_empty() {
                return Ok(());
            }
            let mut application = wake_application.borrow_mut();
            for wake in pending {
                if let Err(message) = apply_wake(&mut *application, document, wake) {
                    application.report_error(document, message);
                }
            }
            Ok(())
        },
    )
}

/// Applies one queued bus wake to the Settings client, reprojecting only the stale section.
fn apply_wake<T: ControlTransport>(
    application: &mut SettingsApplication<T>,
    document: &mut impl UiDocumentAccess,
    wake: SettingsWake,
) -> Result<(), String> {
    match wake {
        SettingsWake::Lost(message) => Err(format!("settings live updates stopped: {message}")),
        SettingsWake::Signal(signal) => {
            match reproject_scope(&signal, known_revisions(application)) {
                Some(scope) => application.reproject(scope, document),
                None => Ok(()),
            }
        }
    }
}

fn known_revisions<T: ControlTransport>(application: &SettingsApplication<T>) -> KnownRevisions {
    let client = application.client();
    KnownRevisions {
        settings: client.settings().map(|snapshot| snapshot.revision),
        displays_generation: client.displays().map(|snapshot| snapshot.generation),
        shortcuts: client.shortcuts().map(|snapshot| snapshot.revision),
        panels: client.panels().map(|snapshot| snapshot.revision),
    }
}

impl<T: ControlTransport> SettingsApplication<T> {
    /// Refreshes one authoritative section named by a bus signal, then projects the document.
    /// It shares the private `settle` path with user actions, so the other sections stay untouched.
    pub(crate) fn reproject(
        &mut self,
        scope: SettingsScope,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let refreshed = match scope {
            SettingsScope::Settings => self.client_mut().refresh_settings(),
            SettingsScope::Displays => self.client_mut().refresh_displays(),
            SettingsScope::Shortcuts => self.client_mut().refresh_shortcuts(),
            SettingsScope::Panels => self.client_mut().refresh_panels(),
        };
        refreshed.map_err(crate::control_error)?;
        self.settle(document)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_updates_enter_through_the_reactor_seam_and_reproject_one_section() {
        let production = include_str!("runtime.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("runtime production source");
        assert!(
            production.contains("run_with_controller_events_role_and_two_readable_fds"),
            "bus signals must wake the window through the reactor seam"
        );
        assert!(
            production.contains("application.reproject(scope"),
            "a bus wake must reproject only the stale section"
        );
        assert!(
            !production.contains("refresh_all"),
            "a bus wake must not refresh every section"
        );
    }

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
    fn background_completion_is_polled_from_reactor_wake_without_input() {
        let production = include_str!("runtime.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("runtime production source");
        assert!(production.contains("WallpaperChooserWake::new()"));
        let wake = production
            .find("move |document| {")
            .expect("reactor wake callback");
        let poll = production[wake..]
            .find("application.poll_background(document)")
            .expect("background result is polled from reactor wake");
        assert!(poll < production[wake..].find("if pending.is_empty()").unwrap());
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
