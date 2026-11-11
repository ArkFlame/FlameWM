//! Watches the session control bus for live settings and display snapshots.

use super::*;

pub(super) fn watch_control_signals(
    reactor: &mut Reactor,
    pending: Rc<RefCell<Vec<ControlSignal>>>,
) {
    let client = match ControlSignalClient::connect_session() {
        Ok(client) => Rc::new(client),
        Err(error) => {
            eprintln!("flamewm-lock: live settings/display updates unavailable: {error}");
            return;
        }
    };
    let watch = client.watch();
    let interest = match (
        watch
            .events
            .contains(flamewm_api::ports::FdEvents::READABLE),
        watch
            .events
            .contains(flamewm_api::ports::FdEvents::WRITABLE),
    ) {
        (true, true) => calloop::Interest::BOTH,
        (false, true) => calloop::Interest::WRITE,
        _ => calloop::Interest::READ,
    };
    let result = reactor.register_raw_fd_with_action(watch.fd, interest, move |_, _| {
        let pending = Rc::clone(&pending);
        if let Err(error) = client.on_ready(|signal| {
            if let signal @ (ControlSignal::SettingsSnapshotChanged { .. }
            | ControlSignal::DisplaysSnapshotChanged { .. }) = signal
            {
                pending.borrow_mut().push(signal);
            }
        }) {
            eprintln!("flamewm-lock: read settings/display update failed: {error}");
        }
        FdAction::Continue
    });
    if let Err(error) = result {
        eprintln!("flamewm-lock: live settings/display watch unavailable: {error}");
    }
}
