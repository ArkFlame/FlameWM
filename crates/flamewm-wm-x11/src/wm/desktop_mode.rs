//! Authoritative desktop CRTC mode and the hold/restore state machine for exclusive
//! fullscreen clients (games that switch the video mode through XRandR or XF86VidMode).
//!
//! The pure state machine has no X calls. The WM glue below reads the live CRTC state only
//! when a RandR topology change is pending or a hold is active, feeds it to
//! [`DesktopModeTracker::observe`], and performs whatever [`ModeAction`] comes back.

use super::*;

/// One active CRTC: enough to re-issue RRSetCrtcConfig for the same output placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrtcMode {
    pub crtc: u32,
    pub x: i16,
    pub y: i16,
    pub mode: u32,
    pub rotation: u16,
    pub outputs: Vec<u32>,
}

/// What the WM must do after observing the live CRTC state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModeAction {
    /// Live state is the desktop state, or a new desktop was adopted: relayout as usual.
    Track,
    /// A fullscreen client changed the mode: keep panels and windows at the desktop geometry.
    Hold,
    /// The fullscreen client is gone (or never restored the mode): re-apply the desktop CRTCs.
    Restore(Vec<CrtcMode>),
}

#[derive(Debug, Default)]
pub(crate) struct DesktopModeTracker {
    desktop: Vec<CrtcMode>,
    /// XRandR screen extent, separate from the active CRTC modes. Exclusive games may resize
    /// the root framebuffer without changing its CRTC identifiers.
    desktop_size: Option<(u16, u16)>,
    borrowed: bool,
    seeded: bool,
}

impl DesktopModeTracker {
    /// True until the first observation: the desktop must be recorded before any client can
    /// change the mode, so the WM seeds it from the live state on its first turn.
    pub(crate) fn needs_seed(&self) -> bool {
        !self.seeded
    }

    /// `live` is the current active CRTC set; `fullscreen_client` is true while any managed
    /// client is fullscreen. Never blocks: it only compares and decides.
    pub(crate) fn observe(
        &mut self,
        live: &[CrtcMode],
        size: (u16, u16),
        fullscreen_client: bool,
    ) -> ModeAction {
        self.seeded = true;
        if self.borrowed {
            if live == self.desktop.as_slice() && Some(size) == self.desktop_size {
                self.borrowed = false;
                return ModeAction::Track;
            }
            if fullscreen_client {
                return ModeAction::Hold;
            }
            // Stay borrowed until the live state matches again, so a failed restore is retried
            // instead of adopting the temporary mode as the new desktop.
            return ModeAction::Restore(self.desktop.clone());
        }
        if self.desktop.is_empty() {
            self.desktop = live.to_vec();
            self.desktop_size = Some(size);
            return ModeAction::Track;
        }
        if live == self.desktop.as_slice() && Some(size) == self.desktop_size {
            return ModeAction::Track;
        }
        if fullscreen_client {
            self.borrowed = true;
            return ModeAction::Hold;
        }
        // A change with no fullscreen client is a settings or hotplug change: it is the new desktop.
        self.desktop = live.to_vec();
        self.desktop_size = Some(size);
        ModeAction::Track
    }

    /// True while a fullscreen client's mode change is being held back.
    pub(crate) fn is_borrowed(&self) -> bool {
        self.borrowed
    }
}

impl<'a, C: Connection> Wm<'a, C> {
    /// Runs once per loop turn. Returns true while the desktop must not follow a temporary mode.
    /// Reads RandR only when a topology change is pending; a failed read holds the desktop and
    /// keeps the change pending so the next turn retries.
    pub(crate) fn observe_desktop_mode(&mut self) -> bool {
        if self.desktop_mode.needs_seed() {
            // Seeding is not a topology change: record whatever is live now as the desktop.
            if let Ok((live, _, size)) = self.read_live_crtcs() {
                self.desktop_mode.observe(&live, size, false);
            }
            return false;
        }
        if !self.display_topology_dirty.is_pending() {
            return self.desktop_mode.is_borrowed();
        }
        let fullscreen = self.clients.values().any(|client| client.is_fullscreen());
        let Ok(live) = self.read_live_crtcs() else {
            self.display_topology_dirty.mark();
            return true;
        };
        match self.desktop_mode.observe(&live.0, live.2, fullscreen) {
            ModeAction::Track => false,
            ModeAction::Hold => true,
            ModeAction::Restore(desktop) => {
                if let Some(size) = self.desktop_mode.desktop_size {
                    match self.restore_crtcs(&desktop, size) {
                        Ok(true) => {}
                        Ok(false) => eprintln!("flamewm: desktop mode restore rejected by XRandR"),
                        Err(error) => eprintln!("flamewm: desktop mode restore failed: {error}"),
                    }
                }
                // Our own RRSetCrtcConfig echoes as a notification; refresh on this turn too.
                self.display_topology_dirty.mark();
                self.desktop_mode.is_borrowed()
            }
        }
    }

    /// Called when a managed client leaves while a mode change is held, so the hold is re-checked.
    pub(crate) fn note_client_gone_for_desktop_mode(&mut self) {
        if self.desktop_mode.is_borrowed() {
            self.display_topology_dirty.mark();
        }
    }

    fn read_live_crtcs(&self) -> Result<(Vec<CrtcMode>, u32, (u16, u16)), ReplyError> {
        let root = self.screen().root;
        let resources = self
            .conn
            .randr_get_screen_resources_current(root)?
            .reply()?;
        let mut live = Vec::new();
        for crtc in resources.crtcs {
            let info = self
                .conn
                .randr_get_crtc_info(crtc, resources.config_timestamp)?
                .reply()?;
            if info.mode == 0 {
                continue;
            }
            live.push(CrtcMode {
                crtc,
                x: info.x,
                y: info.y,
                mode: info.mode,
                rotation: u16::from(info.rotation),
                outputs: info.outputs,
            });
        }
        let geometry = self.conn.get_geometry(root)?.reply()?;
        Ok((
            live,
            resources.config_timestamp,
            (geometry.width, geometry.height),
        ))
    }

    /// Restore both the root framebuffer and CRTC mode/placement. A game may change only the
    /// framebuffer size; RRSetCrtcConfig alone will not undo that and panels stay shrunken.
    fn restore_crtcs(&self, desktop: &[CrtcMode], size: (u16, u16)) -> Result<bool, ReplyError> {
        let root = self.screen().root;
        let geometry = self.conn.get_geometry(root)?.reply()?;
        let current_size = (geometry.width, geometry.height);
        let mm_width = u32::from(self.screen().width_in_millimeters);
        let mm_height = u32::from(self.screen().height_in_millimeters);
        // Expand each axis before moving CRTCs. On mixed-direction changes, setting the final
        // size directly could clip a still-active CRTC in the dimension that must shrink.
        let intermediate = (size.0.max(current_size.0), size.1.max(current_size.1));
        if intermediate != current_size {
            self.conn
                .randr_set_screen_size(root, intermediate.0, intermediate.1, mm_width, mm_height)?
                .check()?;
        }
        for crtc in desktop {
            // Each mode set can change RandR's config timestamp. Refresh before the next CRTC.
            let resources = self
                .conn
                .randr_get_screen_resources_current(root)?
                .reply()?;
            let reply = self
                .conn
                .randr_set_crtc_config(
                    crtc.crtc,
                    CURRENT_TIME,
                    resources.config_timestamp,
                    crtc.x,
                    crtc.y,
                    crtc.mode,
                    randr::Rotation::from(crtc.rotation),
                    &crtc.outputs,
                )?
                .reply()?;
            if u8::from(reply.status) != u8::from(randr::SetConfig::SUCCESS) {
                return Ok(false);
            }
        }
        if intermediate != size {
            // Shrink only after every output has been moved into the final framebuffer.
            self.conn
                .randr_set_screen_size(root, size.0, size.1, mm_width, mm_height)?
                .check()?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crtc(mode: u32) -> CrtcMode {
        CrtcMode {
            crtc: 1,
            x: 0,
            y: 0,
            mode,
            rotation: 1,
            outputs: vec![7],
        }
    }

    #[test]
    fn first_observation_records_desktop_and_tracks() {
        let mut tracker = DesktopModeTracker::default();
        assert_eq!(
            tracker.observe(&[crtc(10)], (1920, 1080), false),
            ModeAction::Track
        );
        assert!(!tracker.is_borrowed());
    }

    #[test]
    fn mode_change_without_fullscreen_client_is_adopted_as_desktop() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        assert_eq!(
            tracker.observe(&[crtc(11)], (1920, 1080), false),
            ModeAction::Track
        );
        assert!(!tracker.is_borrowed());
        // The adopted mode is now the desktop: returning to 10 is a change, not a restore.
        assert_eq!(
            tracker.observe(&[crtc(10)], (1920, 1080), false),
            ModeAction::Track
        );
        assert!(!tracker.is_borrowed());
    }

    #[test]
    fn fullscreen_mode_change_is_held_and_not_adopted() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        assert_eq!(
            tracker.observe(&[crtc(20)], (1920, 1080), true),
            ModeAction::Hold
        );
        assert!(tracker.is_borrowed());
        // Repeated notifications while the game runs keep holding without relayout.
        assert_eq!(
            tracker.observe(&[crtc(20)], (1920, 1080), true),
            ModeAction::Hold
        );
    }

    #[test]
    fn exit_of_fullscreen_client_restores_recorded_desktop() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        tracker.observe(&[crtc(20)], (1920, 1080), true);
        assert_eq!(
            tracker.observe(&[crtc(20)], (1920, 1080), false),
            ModeAction::Restore(vec![crtc(10)])
        );
        assert!(
            tracker.is_borrowed(),
            "stays borrowed until the restore is seen"
        );
        assert_eq!(
            tracker.observe(&[crtc(10)], (1920, 1080), false),
            ModeAction::Track
        );
        assert!(!tracker.is_borrowed());
    }

    #[test]
    fn fullscreen_root_framebuffer_shrink_restores_even_when_crtc_mode_is_unchanged() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        assert_eq!(
            tracker.observe(&[crtc(10)], (1280, 720), true),
            ModeAction::Hold
        );
        assert_eq!(
            tracker.observe(&[crtc(10)], (1280, 720), false),
            ModeAction::Restore(vec![crtc(10)])
        );
        assert_eq!(tracker.desktop_size, Some((1920, 1080)));
        assert_eq!(
            tracker.observe(&[crtc(10)], (1920, 1080), false),
            ModeAction::Track
        );
    }

    #[test]
    fn failed_restore_is_retried_not_adopted() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        tracker.observe(&[crtc(20)], (1920, 1080), true);
        assert!(matches!(
            tracker.observe(&[crtc(20)], (1920, 1080), false),
            ModeAction::Restore(_)
        ));
        // The restore did not take effect: the temporary mode must not become the desktop.
        assert!(matches!(
            tracker.observe(&[crtc(20)], (1920, 1080), false),
            ModeAction::Restore(_)
        ));
    }

    #[test]
    fn game_that_restored_its_own_mode_needs_no_action() {
        let mut tracker = DesktopModeTracker::default();
        tracker.observe(&[crtc(10)], (1920, 1080), false);
        tracker.observe(&[crtc(20)], (1920, 1080), true);
        assert_eq!(
            tracker.observe(&[crtc(10)], (1920, 1080), true),
            ModeAction::Track
        );
        assert!(!tracker.is_borrowed());
    }
}
