//! Event-dispatch and motion-entry probes (diagnostics only).
use super::*;

impl<C: Connection> Wm<'_, C> {
    pub(super) fn probe_event_dispatch(&mut self, event: &Event, received: usize) {
        let Some(pointer) = pointer_ingress(event) else {
            return;
        };
        match pointer.kind {
            "ButtonPress" => {
                self.pointer_ingress_probe = PointerIngressProbe::default();
                let session_client = self.session_client();
                emit_frame_dispatch(|| {
                    format!(
                        "probe=pointer_ingress stage=pre_handle_event boundary=first raw={} event={} child={} root=({}, {}) session_client={session_client:?} pump_received={received}",
                        pointer.kind, pointer.event, pointer.child, pointer.root_x, pointer.root_y
                    )
                });
            }
            "MotionNotify" => {
                self.pointer_ingress_probe.last_motion = Some(pointer);
                if !self.pointer_ingress_probe.motion_dispatched {
                    self.pointer_ingress_probe.motion_dispatched = true;
                    let session_client = self.session_client();
                    emit_frame_dispatch(|| {
                        format!(
                            "probe=pointer_ingress stage=pre_handle_event boundary=first_motion raw={} motion_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?} pump_received={} pump_coalesced={}",
                            pointer.kind,
                            pointer.event,
                            pointer.child,
                            pointer.root_x,
                            pointer.root_y,
                            received,
                            received.saturating_sub(1),
                        )
                    });
                }
            }
            "ButtonRelease" => {
                let last_motion = self.pointer_ingress_probe.last_motion;
                let session_client = self.session_client();
                emit_frame_dispatch(|| {
                    format!(
                        "probe=pointer_ingress stage=pre_handle_event boundary=last raw={} motion_seen_upstream={} release_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?} last_motion={last_motion:?} pump_received={received}",
                        pointer.kind,
                        last_motion.is_some(),
                        pointer.event,
                        pointer.child,
                        pointer.root_x,
                        pointer.root_y,
                    )
                });
                self.pointer_ingress_probe = PointerIngressProbe::default();
            }
            _ => {}
        }
    }

    pub(super) fn probe_motion_entry(&mut self, event: &MotionNotifyEvent) {
        if self.pointer_ingress_probe.motion_handled {
            return;
        }
        self.pointer_ingress_probe.motion_handled = true;
        let session_client = self.session_client();
        emit_frame_dispatch(|| {
            format!(
                "probe=pointer_ingress stage=handle_motion_entry boundary=first_motion raw=MotionNotify motion_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?}",
                event.event, event.child, event.root_x, event.root_y
            )
        });
    }
}
