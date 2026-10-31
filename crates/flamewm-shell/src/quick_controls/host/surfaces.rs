use super::*;

/// Pure helpers for the close-one/close-none contract: `take_close_target`
/// consumes open and yields the single kind to close (None = zero calls).
#[must_use]
pub fn take_close_target(open: Option<QuickControlKind>) -> Option<QuickControlKind> {
    open
}

pub struct HelperSurfaces {
    /// One lazily created surface per kind (see [`Self::slot`]). A surface is
    /// created on its first open and kept until shutdown: close only unmaps,
    /// and destroying a live display surface defers native library unloads.
    pub(crate) handles: [Option<SurfaceHandle>; 5],
    pub open: Option<QuickControlKind>,
    /// Slide armed for the open popup; `Some` closes it through `hide` (slide-out).
    pub slide: Option<SlideConfig>,
}

impl Default for HelperSurfaces {
    fn default() -> Self {
        Self::new()
    }
}

impl HelperSurfaces {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: [None; 5],
            open: None,
            slide: None,
        }
    }

    pub(crate) fn slot(kind: QuickControlKind) -> usize {
        match kind {
            QuickControlKind::Audio => 0,
            QuickControlKind::Network => 1,
            QuickControlKind::Calendar => 2,
            QuickControlKind::Battery => 3,
            QuickControlKind::Weather => 4,
        }
    }

    /// Existing surface for `kind`, if it has been created.
    #[must_use]
    pub fn handle(&self, kind: QuickControlKind) -> Option<SurfaceHandle> {
        self.handles[Self::slot(kind)]
    }

    /// Surface for `kind`, decoding its document and creating it on first use.
    pub fn ensure(
        &mut self,
        runtime: &mut SurfaceRuntime,
        kind: QuickControlKind,
    ) -> Result<SurfaceHandle, String> {
        if let Some(handle) = self.handle(kind) {
            return Ok(handle);
        }
        let (bytes, title) = match kind {
            QuickControlKind::Audio => (AUDIO_ARTIFACT, AUDIO_POPUP_TITLE),
            QuickControlKind::Network => (NETWORK_ARTIFACT, NETWORK_POPUP_TITLE),
            QuickControlKind::Calendar => (CALENDAR_ARTIFACT, CALENDAR_POPUP_TITLE),
            QuickControlKind::Battery => (BATTERY_ARTIFACT, BATTERY_POPUP_TITLE),
            QuickControlKind::Weather => (WEATHER_ARTIFACT, WEATHER_POPUP_TITLE),
        };
        let handle = create_surface(runtime, bytes, title)?;
        self.handles[Self::slot(kind)] = Some(handle);
        Ok(handle)
    }

    /// Close only the currently open surface (open=kind). No-op when none
    /// is open (zero close calls). The real close span wraps the actual
    /// ungrab/unmap turn.
    /// Pure close-target selection: returns the single handle to close,
    /// or None when nothing is open (zero close calls). Keeps the
    /// close-one/close-none contract testable without an X11 runtime.
    pub fn close_handle(&mut self) -> Option<SurfaceHandle> {
        take_close_target(self.open.take()).and_then(|kind| self.handle(kind))
    }

    pub fn close(&mut self, runtime: &mut SurfaceRuntime) {
        let slide = self.slide.take();
        let Some(surface) = self.close_handle() else {
            return;
        };
        let _span = crate::runtime::shell_span("shell.quick.close/ungrab/unmap").start();
        let _ = close_helper(runtime, surface, slide);
    }

    /// Shutdown path: close the open surface (the shared close owner performs
    /// ungrab/unmap/present/flush), then destroy all helpers. Every failure is
    /// diagnosed and returned so the process owner can choose nonzero exit.
    pub fn shutdown(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        let mut first_error = None;
        if let Some(surface) = self.close_handle() {
            let _span = crate::runtime::shell_span("shell.quick.close/ungrab/unmap").start();
            if let Err(error) = close_helper(runtime, surface, None) {
                eprintln!("flamewm-quick-control: popup close during shutdown failed: {error}");
                first_error = Some(UiBackendError::Renderer(error.to_string()));
            }
        }
        for surface in self.handles.iter_mut().filter_map(Option::take) {
            if let Err(error) = runtime.destroy(surface) {
                eprintln!(
                    "flamewm-quick-control: surface destroy during shutdown failed: {error:?}"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

pub(crate) fn create_surface(
    runtime: &mut SurfaceRuntime,
    bytes: &'static [u8],
    title: &str,
) -> Result<SurfaceHandle, String> {
    let document = decode_document(bytes)?;
    runtime
        .create_surface(
            UiTemplate::new(document),
            SurfaceConfig {
                width: 1,
                height: 1,
                title: title.to_owned(),
                role: SurfaceRole::PopupMenu,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .map_err(|error| format!("{error:?}"))
}

/// Parent-side helper: restart the supervisor with the J07 static span
/// (`shell.quick_control.restart`). Failures are nonfatal: the caller logs
/// and keeps the shell running without quick-control popups.
pub fn poll_supervisor_nonfatal(supervisor: &mut QuickControlSupervisor) {
    let _span = crate::runtime::shell_span("shell.quick_control.restart").start();
    match supervisor.poll() {
        Ok(_) => {}
        Err(SupervisorError::RestartBudgetExhausted) => {
            eprintln!(
                "flamewm-shell: quick-control restart budget exhausted; continuing without helper"
            );
        }
        Err(error) => {
            eprintln!("flamewm-shell: quick-control poll: {error}");
        }
    }
}

/// Parent-side open: compute the retained anchor + work area + edge and
/// forward to the supervisor, then return immediately. Nonfatal on
/// failure: the shell loop survives and the popup simply stays closed.
pub fn open_via_supervisor_nonfatal(
    supervisor: &mut QuickControlSupervisor,
    kind: QuickControlKind,
    anchor: flamewm_api::Rect,
    work_area: flamewm_api::Rect,
    panel_edge: flamewm_api::PanelEdge,
    dock: Option<flamewm_api::Rect>,
) {
    let _span = crate::runtime::shell_span("shell.quick_control.open_request").start();
    if let Err(error) = supervisor.open(super::super::protocol::OpenRequest {
        kind,
        anchor,
        work_area,
        panel_edge,
        dock,
        scope: None,
    }) {
        eprintln!("flamewm-shell: quick-control open failed (nonfatal): {error}");
    }
}

pub fn open_via_supervisor_scoped_nonfatal(
    supervisor: &mut QuickControlSupervisor,
    kind: QuickControlKind,
    anchor: flamewm_api::Rect,
    work_area: flamewm_api::Rect,
    panel_edge: flamewm_api::PanelEdge,
    dock: Option<flamewm_api::Rect>,
    scope: super::super::protocol::OpenScope,
) -> bool {
    let _span = crate::runtime::shell_span("shell.quick_control.open_request").start();
    let request = super::super::protocol::OpenRequest {
        kind,
        anchor,
        work_area,
        panel_edge,
        dock,
        scope: None,
    };
    match supervisor.open_scoped(request, scope) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("flamewm-shell: quick-control open failed (nonfatal): {error}");
            false
        }
    }
}
