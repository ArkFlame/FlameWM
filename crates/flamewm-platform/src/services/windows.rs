use flamewm_api::ports::WindowPort;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::wm_features::{FullscreenMonitorSpan, InteractiveMoveResize, RestackMode};
use flamewm_api::{ErrorCode, FlameError, FlameResult, OutputId, Rect, WindowRef};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WindowService {
    cached: Vec<WindowSnapshot>,
    revision: u64,
    generation: u64,
}

impl WindowService {
    #[must_use]
    pub fn cached(&self) -> &[WindowSnapshot] {
        &self.cached
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn refresh<P: WindowPort>(&mut self, port: &P) -> FlameResult<bool> {
        let fresh = port.snapshot()?;
        let filtered: Vec<_> = fresh
            .into_iter()
            .filter(|window| window.reference.is_valid())
            .collect();
        if filtered == self.cached {
            return Ok(false);
        }
        self.cached = filtered;
        self.revision = self.revision.saturating_add(1);
        self.generation = self.generation.saturating_add(1).max(1);
        Ok(true)
    }

    pub fn get<P: WindowPort>(
        &self,
        port: &P,
        reference: WindowRef,
    ) -> FlameResult<WindowSnapshot> {
        validate_reference(reference)?;
        let current = port.get(reference)?;
        if current.reference.generation != reference.generation {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "stale WindowRef generation",
            ));
        }
        Ok(current)
    }

    pub fn work_area<P: WindowPort>(&self, port: &P, reference: WindowRef) -> FlameResult<Rect> {
        self.get(port, reference)?;
        port.work_area(reference)
    }

    pub fn output<P: WindowPort>(&self, port: &P, reference: WindowRef) -> FlameResult<OutputId> {
        self.get(port, reference)?;
        port.output(reference)
    }

    pub fn activate<P: WindowPort>(&self, port: &mut P, reference: WindowRef) -> FlameResult<()> {
        self.get(port, reference)?;
        port.activate(reference)
    }

    pub fn minimize<P: WindowPort>(&self, port: &mut P, reference: WindowRef) -> FlameResult<()> {
        self.get(port, reference)?;
        port.minimize(reference)
    }

    pub fn maximize<P: WindowPort>(&self, port: &mut P, reference: WindowRef) -> FlameResult<()> {
        self.get(port, reference)?;
        port.maximize(reference)
    }

    pub fn restore<P: WindowPort>(&self, port: &mut P, reference: WindowRef) -> FlameResult<()> {
        self.get(port, reference)?;
        port.restore(reference)
    }

    pub fn close<P: WindowPort>(&self, port: &mut P, reference: WindowRef) -> FlameResult<()> {
        self.get(port, reference)?;
        port.close(reference)
    }

    pub fn showing_desktop<P: WindowPort>(&self, port: &P) -> FlameResult<bool> {
        port.showing_desktop()
    }

    pub fn set_showing_desktop<P: WindowPort>(&self, port: &mut P, show: bool) -> FlameResult<()> {
        port.set_showing_desktop(show)
    }

    pub fn restack<P: WindowPort>(
        &self,
        port: &mut P,
        reference: WindowRef,
        mode: RestackMode,
        sibling: Option<WindowRef>,
    ) -> FlameResult<()> {
        self.get(port, reference)?;
        if let Some(sibling) = sibling {
            self.get(port, sibling)?;
        }
        port.restack(reference, mode, sibling)
    }

    pub fn set_fullscreen_monitors<P: WindowPort>(
        &self,
        port: &mut P,
        reference: WindowRef,
        span: FullscreenMonitorSpan,
    ) -> FlameResult<()> {
        self.get(port, reference)?;
        port.set_fullscreen_monitors(reference, span)
    }

    pub fn begin_interactive_move_resize<P: WindowPort>(
        &self,
        port: &mut P,
        request: InteractiveMoveResize,
    ) -> FlameResult<()> {
        let request = request.validate()?;
        self.get(port, request.window)?;
        port.begin_interactive_move_resize(request)
    }

    pub fn cancel_interactive_move_resize<P: WindowPort>(
        &self,
        port: &mut P,
        reference: WindowRef,
    ) -> FlameResult<()> {
        self.get(port, reference)?;
        port.cancel_interactive_move_resize(reference)
    }
}

fn validate_reference(reference: WindowRef) -> FlameResult<()> {
    if reference.is_valid() {
        Ok(())
    } else {
        Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "invalid WindowRef",
        ))
    }
}
