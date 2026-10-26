//! `XSettings` manager ownership and publication over an abstract X11 port.
//!
//! [`acquire_manager`] implements the claim sequence: refuse a foreign owner,
//! create a manager window, obtain a server timestamp, take the selection,
//! verify ownership, announce with a `MANAGER` client message on the root, and
//! publish the first settings blob. All protocol side effects go through
//! [`ManagerPort`]; the X11 adapter lives in `x11.rs`.

use std::collections::BTreeMap;
use std::os::fd::RawFd;

use flamewm_api::{FlameError, FlameResult};

use crate::blob::{PublishedState, encode_blob, next_publication};
use crate::{XSETTINGS_SELECTION, XSettingsInput, XSettingsValue, published_settings};

pub const MANAGER_MESSAGE_ATOM: &str = "MANAGER";
pub const SETTINGS_PROPERTY_ATOM: &str = "_XSETTINGS_SETTINGS";

/// A `SelectionClear` observed on the manager connection. `window` is the
/// window that lost the selection (the manager window when we are the owner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionClear {
    pub window: u32,
    pub selection: String,
}

/// Whether `FlameWM` still owns `_XSETTINGS_S0`. `Lost` is terminal for the
/// process: `FlameWM` never reclaims the selection from a new owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerOwnership {
    Owned,
    Lost,
}

/// Pure ownership transition. Only a clear of [`XSETTINGS_SELECTION`] on
/// `owned_window` while `Owned` moves to `Lost`. Returns the next state and
/// whether this call caused the transition (so the loss is logged once).
#[must_use]
pub fn ownership_after_clear(
    current: ManagerOwnership,
    owned_window: u32,
    clear: &SelectionClear,
) -> (ManagerOwnership, bool) {
    if current == ManagerOwnership::Owned
        && clear.window == owned_window
        && clear.selection == XSETTINGS_SELECTION
    {
        (ManagerOwnership::Lost, true)
    } else {
        (current, false)
    }
}

/// Raw X11 side effects needed to own `_XSETTINGS_S0`. Window and atom ids are
/// opaque `u32` values; `0` means "no owner" in [`ManagerPort::selection_owner`].
pub trait ManagerPort {
    /// Current selection owner window, or `0` when unowned.
    fn selection_owner(&mut self, selection: &str) -> FlameResult<u32>;
    /// Create the (unmapped) manager window that will own the selection.
    fn create_manager_window(&mut self) -> FlameResult<u32>;
    /// Obtain a real server timestamp by generating a property change on `window`.
    fn acquire_timestamp(&mut self, window: u32) -> FlameResult<u32>;
    /// Take ownership of `selection` for `window` at `timestamp`.
    fn set_selection_owner(
        &mut self,
        selection: &str,
        window: u32,
        timestamp: u32,
    ) -> FlameResult<()>;
    /// Send the `XSettings` `MANAGER` client message to the root window.
    fn announce_manager(&mut self, timestamp: u32, selection: &str, window: u32)
    -> FlameResult<()>;
    /// Replace the `_XSETTINGS_SETTINGS` property on the manager window.
    fn write_settings(&mut self, window: u32, blob: &[u8]) -> FlameResult<()>;
    /// Return the next pending `SelectionClear` on the manager connection, or
    /// `None` when the queue is empty. Must not block. Other events are consumed.
    fn next_selection_clear(&mut self) -> FlameResult<Option<SelectionClear>>;
    /// Descriptor of the manager connection. The reactor watches it for read
    /// readiness so a `SelectionClear` is observed without any timer or poll.
    /// `None` only for ports with no pollable descriptor.
    fn wake_fd(&self) -> Option<RawFd>;
}

/// A live `XSettings` manager: owns the selection through its port and tracks the
/// published state so later publications compute serials correctly.
#[derive(Debug)]
pub struct XSettingsManager<P: ManagerPort> {
    port: P,
    window: u32,
    state: PublishedState,
    ownership: ManagerOwnership,
}

impl<P: ManagerPort> XSettingsManager<P> {
    /// Manager window that owns `_XSETTINGS_S0`.
    #[must_use]
    pub fn window(&self) -> u32 {
        self.window
    }

    /// Currently published state (serial and per-setting change serials).
    #[must_use]
    pub fn state(&self) -> &PublishedState {
        &self.state
    }

    /// Borrow the port, e.g. to inspect a fake in tests.
    #[must_use]
    pub fn port(&self) -> &P {
        &self.port
    }

    /// Current ownership of `_XSETTINGS_S0` as last observed by [`Self::drain_events`].
    #[must_use]
    pub fn ownership(&self) -> ManagerOwnership {
        self.ownership
    }

    /// Descriptor the reactor must watch (read readiness) to observe
    /// `SelectionClear` on the manager connection. Registration is owned by the
    /// reactor; this type never polls.
    #[must_use]
    pub fn wake_fd(&self) -> Option<RawFd> {
        self.port.wake_fd()
    }

    /// Reactor entry point, called when [`Self::wake_fd`] is readable. Drains
    /// the manager connection and returns `true` when this call lost the
    /// selection. Publishes nothing.
    pub fn on_manager_readable(&mut self) -> FlameResult<bool> {
        self.drain_events()
    }

    /// Consume pending manager-connection events. A `SelectionClear` for our
    /// window and selection moves `Owned -> Lost` and is logged once; `FlameWM`
    /// does not fight the new owner. Returns `true` when this call lost ownership.
    pub fn drain_events(&mut self) -> FlameResult<bool> {
        let mut lost_now = false;
        while let Some(clear) = self.port.next_selection_clear()? {
            let (next, transitioned) = ownership_after_clear(self.ownership, self.window, &clear);
            self.ownership = next;
            if transitioned {
                lost_now = true;
                eprintln!(
                    "flamewm: lost _XSETTINGS_S0 selection to another manager; XSettings no longer published"
                );
            }
        }
        Ok(lost_now)
    }

    /// Publish `desired` if it differs from the current settings. Pending
    /// events are drained first, so a `SelectionClear` already queued on the
    /// connection stops publication even when no reactor wake has run yet.
    /// Returns `true` when a new serial was written, `false` for a no-op or when
    /// the manager has lost the selection (a lost manager never writes).
    pub fn publish(&mut self, desired: &BTreeMap<String, XSettingsValue>) -> FlameResult<bool> {
        self.drain_events()?;
        if self.ownership != ManagerOwnership::Owned {
            return Ok(false);
        }
        let Some(next) = next_publication(&self.state, desired) else {
            return Ok(false);
        };
        let blob = encode_blob(&next)?;
        self.port.write_settings(self.window, &blob)?;
        self.state = next;
        Ok(true)
    }

    /// Typed republish entry point for the WM settings-change path: publishes
    /// [`published_settings`] for `input` (only on change). Lost-selection
    /// handling is inside [`Self::publish`].
    pub fn republish(&mut self, input: &XSettingsInput) -> FlameResult<bool> {
        self.publish(&published_settings(input))
    }
}

/// Claim `_XSETTINGS_S0` and publish `desired`. Refuses to run when any other
/// owner exists; never steals ownership.
pub fn acquire_manager<P: ManagerPort>(
    mut port: P,
    selection: &str,
    desired: &BTreeMap<String, XSettingsValue>,
) -> FlameResult<XSettingsManager<P>> {
    if port.selection_owner(selection)? != 0 {
        return Err(FlameError::unavailable(
            "XSettings manager already owned; refusing to replace existing owner",
        ));
    }
    let window = port.create_manager_window()?;
    let timestamp = port.acquire_timestamp(window)?;
    port.set_selection_owner(selection, window, timestamp)?;
    if port.selection_owner(selection)? != window {
        return Err(FlameError::unavailable(
            "XSettings selection ownership was not granted to FlameWM",
        ));
    }
    port.announce_manager(timestamp, selection, window)?;
    let mut manager = XSettingsManager {
        port,
        window,
        state: PublishedState::default(),
        ownership: ManagerOwnership::Owned,
    };
    manager.publish(desired)?;
    Ok(manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XSETTINGS_SELECTION;
    use flamewm_api::ErrorCode;

    const FOREIGN_OWNER: u32 = 0x0900_0001;
    const WINDOW: u32 = 0x0400_0002;
    const TIMESTAMP: u32 = 4242;
    const WAKE_FD: RawFd = 7;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        SetOwner { window: u32, timestamp: u32 },
        Announce { timestamp: u32, window: u32 },
        Write { window: u32, len: usize },
    }

    #[derive(Debug, Default)]
    struct FakePort {
        owner: u32,
        set_owner_grants: bool,
        calls: Vec<Call>,
        pending_clears: Vec<SelectionClear>,
    }

    impl FakePort {
        fn free() -> Self {
            Self {
                owner: 0,
                set_owner_grants: true,
                ..Default::default()
            }
        }
    }

    fn clear(window: u32, selection: &str) -> SelectionClear {
        SelectionClear {
            window,
            selection: selection.to_owned(),
        }
    }

    impl ManagerPort for FakePort {
        fn selection_owner(&mut self, selection: &str) -> FlameResult<u32> {
            assert_eq!(selection, XSETTINGS_SELECTION);
            Ok(self.owner)
        }
        fn create_manager_window(&mut self) -> FlameResult<u32> {
            Ok(WINDOW)
        }
        fn acquire_timestamp(&mut self, window: u32) -> FlameResult<u32> {
            assert_eq!(window, WINDOW);
            Ok(TIMESTAMP)
        }
        fn set_selection_owner(
            &mut self,
            _selection: &str,
            window: u32,
            timestamp: u32,
        ) -> FlameResult<()> {
            self.calls.push(Call::SetOwner { window, timestamp });
            if self.set_owner_grants {
                self.owner = window;
            }
            Ok(())
        }
        fn announce_manager(
            &mut self,
            timestamp: u32,
            selection: &str,
            window: u32,
        ) -> FlameResult<()> {
            assert_eq!(selection, XSETTINGS_SELECTION);
            self.calls.push(Call::Announce { timestamp, window });
            Ok(())
        }
        fn write_settings(&mut self, window: u32, blob: &[u8]) -> FlameResult<()> {
            self.calls.push(Call::Write {
                window,
                len: blob.len(),
            });
            Ok(())
        }
        fn next_selection_clear(&mut self) -> FlameResult<Option<SelectionClear>> {
            if self.pending_clears.is_empty() {
                Ok(None)
            } else {
                Ok(Some(self.pending_clears.remove(0)))
            }
        }
        fn wake_fd(&self) -> Option<RawFd> {
            Some(WAKE_FD)
        }
    }

    fn settings() -> BTreeMap<String, XSettingsValue> {
        BTreeMap::from([("Xft/Antialias".to_owned(), XSettingsValue::Integer(1))])
    }

    #[test]
    fn claims_free_selection_then_announces_and_publishes() {
        let manager = acquire_manager(FakePort::free(), XSETTINGS_SELECTION, &settings())
            .expect("free selection is claimed");
        assert_eq!(manager.window(), WINDOW);
        assert_eq!(manager.state().serial, 1);
        assert_eq!(
            manager.port().calls,
            vec![
                Call::SetOwner {
                    window: WINDOW,
                    timestamp: TIMESTAMP,
                },
                Call::Announce {
                    timestamp: TIMESTAMP,
                    window: WINDOW,
                },
                Call::Write {
                    window: WINDOW,
                    len: manager_blob_len(),
                },
            ]
        );
    }

    fn manager_blob_len() -> usize {
        // header(12) + name entry: 4 + 16 padded name, serial 4, value 4
        // "Xft/Antialias" is 13 bytes -> padded 16.
        12 + 4 + 16 + 4 + 4
    }

    #[test]
    fn refuses_foreign_owner_without_side_effects() {
        let port = FakePort {
            owner: FOREIGN_OWNER,
            set_owner_grants: true,
            ..Default::default()
        };
        let error = acquire_manager(port, XSETTINGS_SELECTION, &settings())
            .expect_err("foreign owner must be respected");
        assert_eq!(error.code, ErrorCode::Unavailable);
    }

    #[test]
    fn fails_when_server_does_not_grant_selection() {
        let port = FakePort {
            owner: 0,
            set_owner_grants: false,
            ..Default::default()
        };
        let error = acquire_manager(port, XSETTINGS_SELECTION, &settings())
            .expect_err("lost ownership race must fail");
        assert_eq!(error.code, ErrorCode::Unavailable);
    }

    #[test]
    fn publish_writes_only_on_change_and_bumps_serial() {
        let mut manager =
            acquire_manager(FakePort::free(), XSETTINGS_SELECTION, &settings()).expect("claim");
        assert!(!manager.publish(&settings()).expect("no-op"));
        assert_eq!(manager.port().calls.len(), 3);
        let changed = BTreeMap::from([("Xft/Antialias".to_owned(), XSettingsValue::Integer(0))]);
        assert!(manager.publish(&changed).expect("change"));
        assert_eq!(manager.state().serial, 2);
        assert_eq!(manager.port().calls.len(), 4);
        assert!(matches!(
            manager.port().calls.last(),
            Some(Call::Write { window: WINDOW, .. })
        ));
    }

    fn claimed() -> XSettingsManager<FakePort> {
        acquire_manager(
            FakePort::free(),
            XSETTINGS_SELECTION,
            &published_settings(&XSettingsInput::default()),
        )
        .expect("claim")
    }

    fn light_input() -> XSettingsInput {
        XSettingsInput {
            appearance: flamewm_api::settings::AppearanceMode::Light,
            ..XSettingsInput::default()
        }
    }

    #[test]
    fn selection_clear_for_manager_selection_flips_owned_to_lost_once() {
        assert_eq!(
            ownership_after_clear(
                ManagerOwnership::Owned,
                WINDOW,
                &clear(WINDOW, XSETTINGS_SELECTION)
            ),
            (ManagerOwnership::Lost, true)
        );
        let mut manager = claimed();
        assert_eq!(manager.ownership(), ManagerOwnership::Owned);
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, XSETTINGS_SELECTION));
        assert!(manager.drain_events().expect("drain"));
        assert_eq!(manager.ownership(), ManagerOwnership::Lost);
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, XSETTINGS_SELECTION));
        assert!(!manager.drain_events().expect("second drain"));
        assert_eq!(manager.ownership(), ManagerOwnership::Lost);
    }

    #[test]
    fn unrelated_selection_clear_is_ignored() {
        let mut manager = claimed();
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, "_NET_WM_S0"));
        manager
            .port
            .pending_clears
            .push(clear(0x0400_0099, XSETTINGS_SELECTION));
        assert!(!manager.drain_events().expect("drain"));
        assert_eq!(manager.ownership(), ManagerOwnership::Owned);
        assert_eq!(
            ownership_after_clear(
                ManagerOwnership::Owned,
                WINDOW,
                &clear(WINDOW, "_NET_WM_S0")
            ),
            (ManagerOwnership::Owned, false)
        );
    }

    #[test]
    fn republish_identical_input_writes_nothing() {
        let mut manager = claimed();
        let calls_before = manager.port().calls.len();
        assert!(
            !manager
                .republish(&XSettingsInput::default())
                .expect("republish")
        );
        assert_eq!(manager.port().calls.len(), calls_before);
        assert_eq!(manager.state().serial, 1);
    }

    #[test]
    fn republish_changed_value_bumps_serial_and_writes() {
        let mut manager = claimed();
        let calls_before = manager.port().calls.len();
        assert!(manager.republish(&light_input()).expect("republish"));
        assert_eq!(manager.state().serial, 2);
        assert_eq!(manager.port().calls.len(), calls_before + 1);
        assert!(matches!(
            manager.port().calls.last(),
            Some(Call::Write { window: WINDOW, .. })
        ));
    }

    #[test]
    fn manager_exposes_wake_fd_for_reactor_registration() {
        let manager = claimed();
        assert_eq!(manager.wake_fd(), Some(WAKE_FD));
    }

    #[test]
    fn readable_wake_without_clear_publishes_nothing() {
        let mut manager = claimed();
        let calls_before = manager.port().calls.len();
        assert!(!manager.on_manager_readable().expect("wake"));
        assert_eq!(manager.ownership(), ManagerOwnership::Owned);
        assert_eq!(manager.port().calls.len(), calls_before);
    }

    #[test]
    fn readable_wake_with_clear_marks_lost_then_republish_is_refused() {
        let mut manager = claimed();
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, XSETTINGS_SELECTION));
        assert!(manager.on_manager_readable().expect("wake"));
        assert_eq!(manager.ownership(), ManagerOwnership::Lost);
        let calls_before = manager.port().calls.len();
        assert!(!manager.republish(&light_input()).expect("republish"));
        assert_eq!(manager.port().calls.len(), calls_before);
    }

    #[test]
    fn publish_after_queued_selection_clear_is_refused_without_prior_drain() {
        let mut manager = claimed();
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, XSETTINGS_SELECTION));
        let calls_before = manager.port().calls.len();
        let changed = published_settings(&light_input());
        assert!(!manager.publish(&changed).expect("publish"));
        assert_eq!(manager.port().calls.len(), calls_before);
        assert_eq!(manager.ownership(), ManagerOwnership::Lost);
    }

    #[test]
    fn lost_manager_does_not_republish() {
        let mut manager = claimed();
        manager
            .port
            .pending_clears
            .push(clear(WINDOW, XSETTINGS_SELECTION));
        let calls_before = manager.port().calls.len();
        assert!(!manager.republish(&light_input()).expect("republish"));
        assert_eq!(manager.ownership(), ManagerOwnership::Lost);
        assert_eq!(manager.port().calls.len(), calls_before);
        assert_eq!(manager.state().serial, 1);
    }
}
