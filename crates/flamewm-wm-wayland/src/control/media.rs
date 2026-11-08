//! Media keys, audio and network drains, and the `NetworkManager` action router.

use super::*;

/// Routes one network action to the `NetworkManager` slot. An absent or unreachable provider answers
/// with its own `Unavailable` error; no device is ever fabricated for the shell.
pub(super) fn network_action(
    network: &RefCell<NetworkManagerSlot>,
    action: SystemAction,
    snapshot: &SystemSnapshot,
) -> FlameResult<()> {
    network
        .borrow_mut()
        .perform_action(action, &snapshot.network)
}

/// Publishes the audio service as unavailable so the shell shows the absence, not an empty list.
pub(super) fn mark_audio_unavailable(host: &mut PlatformHost<WaylandDesktop>) {
    host.system_mut().update_audio(AudioSnapshot {
        availability: ServiceAvailability::Unavailable,
        ..AudioSnapshot::default()
    });
}

impl Service {
    /// Alt+Super+D: flips `dock.autoHide` through the same settings path as the X11 flush.
    pub fn toggle_dock_auto_hide(&self) {
        if let Err(error) = self.host.borrow_mut().toggle_dock_auto_hide() {
            eprintln!("flamewm-wayland: dock auto-hide toggle was not applied: {error:?}");
        }
    }

    /// One panel-brightness media key: align the host fence with the device, step the backlight,
    /// then publish the observed level so the shell OSD shows the new card in the same turn.
    pub fn media_brightness_key(&self, up: bool) {
        let observed = self.backlight.borrow_mut().snapshot();
        self.host
            .borrow_mut()
            .system_mut()
            .update_brightness(observed);
        let key = if up {
            MediaBrightnessKey::Up
        } else {
            MediaBrightnessKey::Down
        };
        let applied = self.host.borrow_mut().media_brightness_key(key);
        if let Err(error) = applied {
            eprintln!("flamewm-wayland: brightness key {key:?} was not applied: {error:?}");
            // The pre-key refresh may still have observed a real level change.
            self.publish_system();
            return;
        }
        // Re-read instead of assuming the write landed: a device that clamps or refuses the step
        // is still reported honestly, and the OSD only moves when the level really changed.
        let observed = self.backlight.borrow_mut().snapshot();
        self.host
            .borrow_mut()
            .system_mut()
            .update_brightness(observed);
        self.publish_system();
    }

    /// One volume media key, capped by the shared "raise maximum volume" setting.
    pub fn media_audio_key(&self, key: MediaAudioKey) {
        let cap = volume_cap(&self.host.borrow().settings_snapshot());
        if let Err(error) = self.host.borrow_mut().media_audio_key(key, cap) {
            eprintln!("flamewm-wayland: audio key {key:?} was not applied: {error:?}");
        }
    }

    /// Applies audio snapshots queued by the helper's reader thread and announces a real change.
    pub(super) fn drain_audio(&self) {
        let updates = {
            let mut pending = self
                .audio_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *pending)
        };
        if updates.is_empty() {
            return;
        }
        let changed = {
            let mut host = self.host.borrow_mut();
            let mut changed = false;
            for update in updates {
                changed |= host.system_mut().update_audio(update);
            }
            changed
        };
        if changed {
            self.publish_system();
        }
    }

    /// Dispatches `NetworkManager` bus traffic and publishes its snapshot when it changed. Runs from
    /// the provider's own fd readiness, and once at install for the initial reconcile.
    pub(super) fn drain_network(&self) {
        let snapshot = {
            let mut network = self.network.borrow_mut();
            if let Err(error) = network.on_ready() {
                eprintln!("flamewm-wayland: NetworkManager dispatch failed: {error}");
            }
            if let Err(error) = network.reconcile() {
                eprintln!("flamewm-wayland: NetworkManager reconcile failed: {error}");
            }
            network.snapshot()
        };
        if self.host.borrow_mut().system_mut().update_network(snapshot) {
            self.publish_system();
        }
    }
}
