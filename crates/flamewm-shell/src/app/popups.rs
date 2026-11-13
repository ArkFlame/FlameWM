//! Quick-control and popup lifecycle: open/close, popup generation and anchor fallback.

use super::*;

impl ShellLoop {
    /// C04: send quick-control CLOSE; close signals are owned in main
    /// (workspace change, menus, media open, outside release, geometry
    /// invalidation). Best-effort and nonfatal: the loop always survives.
    pub(crate) fn close_quick_control_nonfatal(&mut self) {
        self.quick_audio_open = false;
        self.quick_kind_open = None;
        let close = self.popup_session.active().and_then(|lease| {
            is_quick_popup_kind(&lease.kind).then_some(self.popup_session.close(lease.sequence))
        });
        let result = match close {
            Some(flamewm_shell_core::popup_session::PopupTransition::Close(lease)) => {
                let transition =
                    flamewm_shell_core::popup_session::PopupTransition::Close(lease.clone());
                let sequence = lease.sequence;
                let result = self.quick_control.close_session(sequence);
                if result.is_ok() {
                    self.popup_session.commit(&transition);
                }
                result
            }
            _ => self.quick_control.close(),
        };
        if let Err(error) = result {
            let message = error.to_string();
            // NotRunning = no helper/harmless; log the rest as diagnostics.
            if !message.contains("not running") {
                eprintln!("flamewm-shell: quick-control close failed (nonfatal): {message}");
            }
        }
    }

    /// J07: retained anchor + work area + edge -> supervisor.open, then
    /// return immediately. Failures are nonfatal (logged, loop survives).
    /// F10: always reads the current context generation first and
    /// refuses when edge/geometry is inconsistent.
    pub(crate) fn open_quick_control(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
        target: flamewm_shell_core::popup_session::PopupTarget,
    ) {
        use flamewm_shell::quick_controls::QuickControlKind;
        self.note_popup_generation("quick");
        if self.refuse_popup_on_inconsistent_geometry().is_some() {
            return;
        }
        let kind = match name {
            "volume" | "audio" => QuickControlKind::Audio,
            "network" => QuickControlKind::Network,
            "clock" => QuickControlKind::Calendar,
            "battery" => QuickControlKind::Battery,
            "weather" => QuickControlKind::Weather,
            _ => return,
        };
        // Shell-owned and helper-owned popups share one active session.
        if self.surfaces.is_start_group_open()
            || self.surfaces.is_media_open()
            || self.surfaces.is_usage_open()
            || take_context_menu_for_popup(&mut self.context_menu)
        {
            if let Err(error) = self.close_transients(runtime) {
                eprintln!("flamewm-shell: close shell popup for quick popup failed: {error:?}");
                return;
            }
        }
        let output = target.output.clone();
        let transition = self
            .popup_session
            .open(quick_popup_kind(kind), target.clone())
            .ok();
        let Some(transition) = transition else { return };
        let sequence = match &transition {
            flamewm_shell_core::popup_session::PopupTransition::Open(lease) => lease.sequence,
            flamewm_shell_core::popup_session::PopupTransition::Replace { next, .. } => {
                next.sequence
            }
            flamewm_shell_core::popup_session::PopupTransition::Keep => return,
            flamewm_shell_core::popup_session::PopupTransition::Close(_) => return,
        };
        let source_id = match kind {
            QuickControlKind::Audio => "tray-volume",
            QuickControlKind::Network => "tray-network",
            QuickControlKind::Calendar => "clock-button",
            QuickControlKind::Battery => "tray-battery",
            QuickControlKind::Weather => "tray-weather",
        };
        // Retained anchor preferred; snapshot geometry is the sole
        // pre-map fallback (same contract as the in-process path).
        let Some(fallback) = self.pending_anchor_fallback(&output) else {
            return;
        };
        let Some(anchor) = self
            .surfaces
            .quick_anchor_for_output(runtime, &output, &fallback, source_id)
        else {
            return;
        };
        let Some(placement) = self.surfaces.placement_for_output(&output) else {
            return;
        };
        // Panel-aware work area: quick popups pin right to the output and fit
        // vertically inside it, so they never cover the panel strip.
        let work_area = placement.work_area;
        let panel_edge = placement.panel.edge;
        // Retained panel rect: the helper clips slide transit frames against the dock.
        let dock = self
            .surfaces
            .panel_for_output(&output)
            .and_then(|panel| runtime.surface_device_rect(panel).ok())
            .map(|r| {
                flamewm_api::Rect::new(r.x as i32, r.y as i32, r.width as i32, r.height as i32)
            });
        if kind == QuickControlKind::Weather {
            // Fetch on this open; the feed goes out before OPEN on the same ordered channel.
            self.weather.request_open_fetch();
            self.push_weather_feed();
        }
        let sent = flamewm_shell::quick_controls::host::open_via_supervisor_scoped_nonfatal(
            &mut self.quick_control,
            kind,
            anchor,
            work_area,
            panel_edge,
            dock,
            flamewm_shell::quick_controls::protocol::OpenScope {
                output,
                topology_generation: target.topology_generation,
                sequence,
            },
        );
        if sent {
            self.popup_session.commit(&transition);
            self.quick_audio_open = kind == QuickControlKind::Audio;
            self.quick_kind_open = Some(kind);
            runtime.arm_raw_press_observer();
        }
    }

    /// Popup group that an outside-press dismissal would close right now.
    pub(crate) fn open_popup_group(&self) -> Option<&'static str> {
        if self.surfaces.is_start_group_open() {
            return Some("start");
        }
        if self.surfaces.is_media_open() {
            return Some("media");
        }
        if self.surfaces.is_usage_open() {
            return Some("usage");
        }
        self.quick_kind_open.map(quick_popup_group)
    }

    /// Pre-map anchor fallback: the panel snapshot geometry. Used only
    /// when no retained layout exists yet (same sole-fallback contract as
    /// the in-process popup path).
    pub(crate) fn pending_anchor_fallback(
        &self,
        output: &flamewm_api::OutputId,
    ) -> Option<flamewm_api::Rect> {
        self.surfaces
            .placement_for_output(output)
            .map(|placement| placement.panel.geometry)
    }

    /// F10: edge/geometry gate shared by every popup open path.
    pub(crate) fn refuse_popup_on_inconsistent_geometry(&self) -> Option<String> {
        match self.surfaces.check_edge_consistent() {
            Ok(()) => None,
            Err(diagnostic) => {
                debug_assert!(false, "{diagnostic}");
                eprintln!("flamewm-shell: popup open refused: {diagnostic}");
                Some(diagnostic)
            }
        }
    }

    /// F10: read the current context generation on every Start/Quick
    /// open so the open provably uses the post-sync context.
    pub(crate) fn note_popup_generation(&self, what: &'static str) {
        let (generation, revision) = self.surfaces.placement_generation();
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("shell.popup.open.generation"),
            Duration::from_millis(500),
            || format!("F10 {what} generation={generation} revision={revision}"),
        );
    }

    /// Name of the currently open in-process status popup, if any.
    /// Helper-owned popups are never in-process, so only media qualifies.
    pub(crate) fn open_popup_name(&self) -> Option<&'static str> {
        if self.surfaces.is_media_open() {
            return Some("media");
        }
        if self.surfaces.is_usage_open() {
            return Some("usage");
        }
        None
    }
}

/// Whether a quick-control popup's taskbar item is still shown under `items`.
pub(super) fn quick_kind_shown(
    kind: flamewm_shell::quick_controls::QuickControlKind,
    items: flamewm_shell::projection::TaskbarItems,
) -> bool {
    use flamewm_shell::quick_controls::QuickControlKind;
    match kind {
        QuickControlKind::Audio => items.volume,
        QuickControlKind::Network => items.network,
        QuickControlKind::Battery => items.battery,
        QuickControlKind::Weather => items.weather,
        QuickControlKind::Calendar => items.clock,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quick_open_ack_commits_only_the_matching_lease_and_failures_release_it() {
        use flamewm_shell::quick_controls::protocol::OpenResult;
        use flamewm_shell_core::popup_session::{PopupKind, PopupSession, PopupTarget};
        let target = || PopupTarget {
            output: flamewm_api::OutputId::new("A"),
            topology_generation: 3,
            panels_revision: 6,
        };
        let mut presented = PopupSession::default();
        let open = presented.open(PopupKind::Audio, target()).unwrap();
        assert!(presented.commit(&open));
        let sequence = presented.active().unwrap().sequence;
        assert_eq!(
            super::apply_quick_open_result(&mut presented, OpenResult::Presented(sequence)),
            super::QuickOpenResolution::Presented,
        );
        assert_eq!(presented.active().unwrap().sequence, sequence);

        for failure in [
            OpenResult::Refused(sequence),
            OpenResult::RolledBack(sequence),
            OpenResult::HostExited(sequence),
        ] {
            let mut session = PopupSession::default();
            let open = session.open(PopupKind::Audio, target()).unwrap();
            assert!(session.commit(&open));
            assert_eq!(
                super::apply_quick_open_result(&mut session, failure),
                super::QuickOpenResolution::Failed,
            );
            assert!(session.active().is_none());
        }
    }

    #[test]
    fn stale_quick_open_result_cannot_clear_a_newer_popup_lease() {
        use flamewm_shell::quick_controls::protocol::OpenResult;
        use flamewm_shell_core::popup_session::{PopupKind, PopupSession, PopupTarget};
        let target = PopupTarget {
            output: flamewm_api::OutputId::new("A"),
            topology_generation: 3,
            panels_revision: 6,
        };
        let mut session = PopupSession::default();
        let old = session.open(PopupKind::Audio, target.clone()).unwrap();
        assert!(session.commit(&old));
        let old_sequence = session.active().unwrap().sequence;
        let next = session.open(PopupKind::Network, target).unwrap();
        assert!(session.commit(&next));
        let current = session.active().cloned().unwrap();
        assert_eq!(
            super::apply_quick_open_result(&mut session, OpenResult::Refused(old_sequence)),
            super::QuickOpenResolution::Stale,
        );
        assert_eq!(session.active(), Some(&current));
    }

    #[test]
    fn open_media_popup_closes_when_media_unavailable_or_idle() {
        use flamewm_api::system::{PlaybackState, ServiceAvailability, SystemSnapshot};
        let mut system = SystemSnapshot::default();
        system.media.availability = ServiceAvailability::Unavailable;
        system.media.playback = PlaybackState::Playing;
        assert!(
            super::media_popup_must_close(&system, true),
            "player gone must close the open media popup"
        );
        system.media.availability = ServiceAvailability::Available;
        system.media.playback = PlaybackState::Unavailable;
        assert!(
            super::media_popup_must_close(&system, true),
            "no playing/paused media must close the open media popup"
        );
        system.media.playback = PlaybackState::Playing;
        assert!(
            !super::media_popup_must_close(&system, true),
            "playing media keeps the popup open"
        );
        assert!(
            !super::media_popup_must_close(&system, false),
            "closed popup is never closed again"
        );
    }

    #[test]
    fn quick_popup_open_takes_and_closes_open_context_menu_only() {
        let mut context_menu = Some(flamewm_shell::ContextMenuState {
            kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
            revision: 1,
            output: None,
            interaction: None,
            anchor: None,
        });
        assert!(super::take_context_menu_for_popup(&mut context_menu));
        assert!(context_menu.is_none(), "popup open releases the menu slot");
        assert!(
            !super::take_context_menu_for_popup(&mut context_menu),
            "no surface close is issued when no context menu is open"
        );
    }
}
