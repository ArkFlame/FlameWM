use flamewm_reactor::Reactor;
use flamewm_render_x11::SurfaceControllerEvent;

use crate::{SurfaceEvent, SurfaceRuntime, UiBackendError};

pub fn pump_events<F>(
    runtime: &mut SurfaceRuntime,
    mut on_event: F,
) -> Result<usize, UiBackendError>
where
    F: FnMut(SurfaceEvent),
{
    runtime
        .controller
        .pump_events(|event: &SurfaceControllerEvent, _document| {
            on_event(to_surface_event(event));
            Ok(())
        })
        .map_err(UiBackendError::Renderer)
}

fn to_surface_event(event: &SurfaceControllerEvent) -> SurfaceEvent {
    SurfaceEvent {
        surface: crate::SurfaceHandle::from_id(event.surface),
        action: event.event.clone(),
    }
}

/// Block on the shared reactor, pump X events when the connection is
/// ready, run one tick callback, then redraw dirty surfaces. The X
/// connection fd is registered without ownership transfer; the caller
/// keeps the runtime (and its display) alive until this returns.
///
/// X behavior (grab/outside/pump) stays owned by `SurfaceController`;
/// this runner only wires reactor readiness to that existing pump.
pub fn run_surface_runtime_with_reactor<F, T>(
    runtime: &mut SurfaceRuntime,
    reactor: &mut Reactor,
    mut on_event: F,
    mut on_tick: T,
) -> Result<(), UiBackendError>
where
    F: FnMut(SurfaceEvent),
    T: FnMut() -> Result<(), String>,
{
    runtime
        .controller
        .run_with_reactor(
            reactor,
            |event: &SurfaceControllerEvent, _document| {
                on_event(to_surface_event(event));
                Ok(())
            },
            |_controller| on_tick(),
        )
        .map_err(UiBackendError::Renderer)
}

/// Run reactor cycles while keeping native document borrows inside the renderer.
///
/// Each queued event is delivered with a mutable [`SurfaceRuntime`] after native dispatch ends;
/// tick work uses the same controlled runtime boundary.
pub fn run_surface_runtime_with_reactor_access<F, T>(
    runtime: &mut SurfaceRuntime,
    reactor: &mut Reactor,
    on_event: F,
    on_tick: T,
) -> Result<(), UiBackendError>
where
    F: FnMut(SurfaceEvent, &mut SurfaceRuntime) -> Result<(), UiBackendError>,
    T: FnMut(&mut SurfaceRuntime) -> Result<(), UiBackendError>,
{
    run_surface_runtime_until(runtime, reactor, on_event, on_tick, |runtime| {
        runtime.has_surfaces()
    })
}

/// Same loop as [`run_surface_runtime_with_reactor_access`], but the loop stops when
/// `keep_running` returns false instead of when no surfaces remain. Hosts that create
/// surfaces lazily must use this: with zero surfaces the default predicate exits at once.
pub fn run_surface_runtime_until<F, T, K>(
    runtime: &mut SurfaceRuntime,
    reactor: &mut Reactor,
    mut on_event: F,
    mut on_tick: T,
    mut keep_running: K,
) -> Result<(), UiBackendError>
where
    F: FnMut(SurfaceEvent, &mut SurfaceRuntime) -> Result<(), UiBackendError>,
    T: FnMut(&mut SurfaceRuntime) -> Result<(), UiBackendError>,
    K: FnMut(&SurfaceRuntime) -> bool,
{
    loop {
        let (events, drained) = runtime
            .controller
            .run_with_reactor_step(reactor)
            .map_err(UiBackendError::Renderer)?;
        for event in events {
            on_event(
                SurfaceEvent {
                    surface: crate::SurfaceHandle::from_id(event.surface),
                    action: event.event,
                },
                runtime,
            )?;
        }
        if !keep_running(runtime) {
            break;
        }
        on_tick(runtime)?;
        if drained > 0 && runtime.has_surfaces() {
            runtime.redraw_dirty()?;
        }
        if !keep_running(runtime) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn surface_runtime_runner_signature_pins_reactor_contract() {
        // Runtime behavior needs X; this pins the generic runner signature.
        let _ = super::run_surface_runtime_with_reactor::<
            fn(super::SurfaceEvent),
            fn() -> Result<(), String>,
        >;
    }

    #[test]
    fn surface_runtime_until_runner_keeps_lazy_hosts_alive_without_surfaces() {
        // Zero surfaces is a valid idle state for lazily created hosts; the
        // until-runner must accept an explicit keep-running predicate.
        let _ = super::run_surface_runtime_until::<
            fn(
                super::SurfaceEvent,
                &mut super::SurfaceRuntime,
            ) -> Result<(), super::UiBackendError>,
            fn(&mut super::SurfaceRuntime) -> Result<(), super::UiBackendError>,
            fn(&super::SurfaceRuntime) -> bool,
        >;
    }

    #[test]
    fn surface_runtime_access_runner_signature_pins_controlled_boundary() {
        let _ = super::run_surface_runtime_with_reactor_access::<
            fn(
                super::SurfaceEvent,
                &mut super::SurfaceRuntime,
            ) -> Result<(), super::UiBackendError>,
            fn(&mut super::SurfaceRuntime) -> Result<(), super::UiBackendError>,
        >;
    }
}
