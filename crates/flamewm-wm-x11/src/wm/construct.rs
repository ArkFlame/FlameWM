//! Wm construction and the become-WM handshake.
use super::*;

/// Failure record for a degraded startup stage. It keeps the exact operator-facing
/// text so the fallback decision can be pure and tested without an X server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupFault {
    pub(crate) stage: &'static str,
    pub(crate) message: String,
    /// `Some` only for a fatal stage; `run_with_hook` propagates it to `exit(1)`.
    pub(crate) exit_code: Option<i32>,
}

/// RandR query outcome, reduced from the extension cookie reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RandrQuery {
    Present,
    Absent,
    Failed(String),
}

/// Pure: an absent or failed optional extension degrades with a failure record.
pub(crate) fn missing_extension_fault(extension: &str, detail: &str) -> StartupFault {
    StartupFault {
        stage: "extension",
        message: format!("flamewm: {extension} absent; blast radius: {detail}"),
        exit_code: None,
    }
}

/// Optional rendering extensions queried once at startup. Each absent extension already
/// degrades to the plain core-protocol path in the renderer/WM; the audit here makes that
/// degradation observable as exactly one failure record per extension.
pub(crate) const CORE_FALLBACK_EXTENSIONS: &[(&[u8], &str, &str)] = &[
    (
        b"RENDER",
        "RENDER",
        "XRender-based text and image drawing falls back to core X",
    ),
    (
        b"COMPOSITE",
        "COMPOSITE",
        "frames are not redirected; transitions may be missing",
    ),
    (
        b"DAMAGE",
        "DAMAGE",
        "window-transition damage tracking is disabled",
    ),
    (
        b"XFIXES",
        "XFIXES",
        "not used by the WM sources; reported for completeness",
    ),
];

/// Pure reduction of one optional-extension query to a single failure record.
pub(crate) fn core_fallback_fault(name: &str, detail: &str, present: bool) -> Option<StartupFault> {
    (!present).then(|| missing_extension_fault(name, detail))
}

/// Outcome of one optional-extension query. A failed query is `Unknown`, never `Absent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtensionQuery {
    Present,
    Absent,
    Unknown(String),
}

/// Pure: an optional extension yields a record only when it is absent or its query failed.
/// A failed query must not be reported as a missing extension.
pub(crate) fn optional_extension_fault(
    name: &str,
    detail: &str,
    query: ExtensionQuery,
) -> Option<StartupFault> {
    match query {
        ExtensionQuery::Present => None,
        ExtensionQuery::Absent => Some(missing_extension_fault(name, detail)),
        ExtensionQuery::Unknown(error) => Some(StartupFault {
            stage: "extension-query",
            message: format!(
                "flamewm: {name} query failed: {error}; support is unknown; blast radius: {detail}"
            ),
            exit_code: None,
        }),
    }
}

/// Pure: the decoration renderer failed to open. Decorations and cursors use core frames.
pub(crate) fn renderer_fault(error: &str) -> StartupFault {
    StartupFault {
        stage: "renderer",
        message: format!(
            "flamewm: decoration renderer unavailable: {error}; blast radius: decorations and cursors use core frames"
        ),
        exit_code: None,
    }
}

fn query_optional_extension<C: Connection>(conn: &C, name: &[u8]) -> ExtensionQuery {
    match conn
        .query_extension(name)
        .map_err(ReplyError::from)
        .and_then(|cookie| cookie.reply())
    {
        Ok(reply) if reply.present => ExtensionQuery::Present,
        Ok(_) => ExtensionQuery::Absent,
        Err(error) => ExtensionQuery::Unknown(error.to_string()),
    }
}

/// Pure: resolve the RandR event stage to live events or a root-geometry fallback,
/// recording the reason whenever it degrades.
pub(crate) fn randr_event_fault(
    query: RandrQuery,
    selection_error: Option<String>,
) -> Option<StartupFault> {
    match query {
        RandrQuery::Present if selection_error.is_none() => None,
        RandrQuery::Present => selection_error.map(|error| StartupFault {
            stage: "randr-events",
            message: format!(
                "flamewm: RandR event selection failed: {error}; blast radius: no live output hotplug/CRTC change notifications, root geometry is used until restart"
            ),
            exit_code: None,
        }),
        RandrQuery::Absent => Some(missing_extension_fault(
            "RandR",
            "no live output hotplug/CRTC change notifications, root geometry is used",
        )),
        RandrQuery::Failed(error) => Some(StartupFault {
            stage: "randr-events",
            message: format!(
                "flamewm: RandR query failed: {error}; blast radius: no live output hotplug/CRTC change notifications, root geometry is used"
            ),
            exit_code: None,
        }),
    }
}

/// Pure: a second WM holding the root `SubstructureRedirect` selection is fatal.
pub(crate) fn second_wm_fault(error_kind: Option<ErrorKind>) -> Option<StartupFault> {
    (error_kind == Some(ErrorKind::Access)).then_some(StartupFault {
        stage: "root-selection",
        message: "flamewm: another window manager is already running on this display; blast radius: this session has no WM and exits 1".into(),
        exit_code: Some(1),
    })
}

/// Optional input/frame extensions queried once at startup. Each degrades on its own;
/// absence is reported but never fatal (xfwm4 display.c queries each independently).
pub(crate) const INPUT_FALLBACK_EXTENSIONS: &[(&[u8], &str, &str)] = &[
    (
        b"XInputExtension",
        "XInputExtension",
        "raw escape-key, tablet and touch events are unavailable; core key events still work",
    ),
    (
        b"SHAPE",
        "SHAPE",
        "input-shape masks for frames and snap previews are disabled",
    ),
    (
        b"XKB",
        "XKB",
        "no XKB layout/indicator state; core keycodes and key events still work",
    ),
];

/// One consolidated startup report: every degraded stage is collected first, then
/// rendered once with its blast radius, so operators see a single coherent message.
#[derive(Debug, Default)]
pub(crate) struct StartupReport {
    faults: Vec<StartupFault>,
}

impl StartupReport {
    pub(crate) fn push(&mut self, fault: StartupFault) {
        self.faults.push(fault);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.faults.is_empty()
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::from("flamewm: startup degradation report (blast radii):");
        for fault in &self.faults {
            out.push_str("\n- ");
            out.push_str(&fault.message);
        }
        out
    }
}

/// Pure: the X connection is gone. Fatal, but reported through the same report
/// path so the failure is explicit rather than a panic.
pub(crate) fn connection_lost_fault(detail: &str) -> StartupFault {
    StartupFault {
        stage: "connection",
        message: format!(
            "flamewm: X connection lost: {detail}; blast radius: WM cannot operate; exiting cleanly"
        ),
        exit_code: Some(1),
    }
}

pub(super) fn become_wm<C: Connection>(conn: &C, screen: &Screen) -> Result<(), ReplyError> {
    let attrs = ChangeWindowAttributesAux::new().event_mask(super::hot_corner::root_event_mask());
    let mut report = StartupReport::default();
    let result = match conn.change_window_attributes(screen.root, &attrs) {
        Ok(cookie) => cookie.check(),
        Err(error) => {
            report.push(connection_lost_fault(error.to_string().as_str()));
            Err(ReplyError::ConnectionError(error))
        }
    };
    // A lost connection cannot answer extension queries; stop here so no false "absent" records follow.
    if matches!(result, Err(ReplyError::ConnectionError(_))) {
        if !report.is_empty() {
            eprintln!("{}", report.render());
        }
        return result;
    }
    // RandR is optional: a server without it still gets a working WM. Losing this
    // selection means no live SCREEN_CHANGE/OUTPUT_CHANGE/CRTC_CHANGE notifications;
    // output geometry falls back to the root window until the next start.
    let query = match conn
        .query_extension(b"RANDR")
        .map_err(ReplyError::from)
        .and_then(|cookie| cookie.reply())
    {
        Ok(reply) if reply.present => RandrQuery::Present,
        Ok(_) => RandrQuery::Absent,
        Err(error) => RandrQuery::Failed(error.to_string()),
    };
    let selection_error = (query == RandrQuery::Present)
        .then(|| {
            conn.randr_select_input(
                screen.root,
                randr::NotifyMask::SCREEN_CHANGE
                    | randr::NotifyMask::OUTPUT_CHANGE
                    | randr::NotifyMask::CRTC_CHANGE,
            )
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.check())
            .err()
            .map(|error| error.to_string())
        })
        .flatten();
    if let Some(fault) = randr_event_fault(query, selection_error) {
        report.push(fault);
    }
    // RENDER/COMPOSITE/DAMAGE/XFIXES are optional. The renderer and WM already fall back to
    // the plain core-protocol path when one is absent; report each absence once so the
    // degraded session is observable rather than silent.
    for &(name, display_name, detail) in CORE_FALLBACK_EXTENSIONS {
        let query = query_optional_extension(conn, name);
        if let Some(fault) = optional_extension_fault(display_name, detail, query) {
            report.push(fault);
        }
    }
    for &(name, display_name, detail) in INPUT_FALLBACK_EXTENSIONS {
        let query = query_optional_extension(conn, name);
        if let Some(fault) = optional_extension_fault(display_name, detail, query) {
            report.push(fault);
        }
    }

    let selection_kind = match &result {
        Err(ReplyError::X11Error(error)) => Some(error.error_kind),
        _ => None,
    };
    if let Some(fault) = second_wm_fault(selection_kind) {
        report.push(fault);
    }
    if !report.is_empty() {
        eprintln!("{}", report.render());
    }
    result
}

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn new(
        conn: &'a C,
        screen_num: usize,
        config: WmConfig,
        catalog: std::sync::Arc<flamewm_applications::ApplicationCatalog>,
        icon_service: Rc<RefCell<flamewm_integrations_linux::icon_service::IconService>>,
    ) -> Result<Self, AnyError> {
        let screen = &conn.setup().roots[screen_num];
        let atoms = Atoms::new(conn)?;

        let support_window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            support_window,
            screen.root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;

        let workspace_count = config.workspaces.max(1);
        let window_config = std::rc::Rc::new(std::cell::RefCell::new(
            crate::window_config::WindowConfig::load(
                crate::runtime::settings_path()
                    .0
                    .parent()
                    .map(std::path::Path::to_path_buf),
            ),
        ));
        let mut renderer_report = StartupReport::default();
        let renderer = match flamewm_render_x11::ExternalDecorationRenderer::open("IBM Plex Sans") {
            Ok(renderer) => Some(renderer.into_cursor_shared()),
            Err(error) => {
                renderer_report.push(renderer_fault(&error));
                None
            }
        };
        if !renderer_report.is_empty() {
            eprintln!("{}", renderer_report.render());
        }
        // Root gets the theme cursor at start; the renderer records it, so a live
        // cursor-theme reload re-defines it with the frames.
        if let Some(renderer) = renderer.as_ref() {
            let _ = renderer.borrow_mut().define_cursor(
                u64::from(screen.root),
                flamewm_render_core::CursorKind::Default,
            );
        }
        // A compositing manager with a 32-bit visual gets the ARGB ring: it is composited over the
        // client's own pixels, so the arcs keep their antialiased coverage. Every other session
        // keeps the frame-band ring fallback.
        let outline_overlay_available = renderer
            .as_ref()
            .is_some_and(|renderer| renderer.borrow().outline_overlay_available());
        let animator = Animator::new(
            renderer.as_ref().is_some_and(|r| {
                r.borrow().is_available() && r.borrow().supports_picture_transform()
            }),
            Settings::DEFAULT,
        );
        Ok(Self {
            conn,
            screen_num,
            config,
            atoms,
            support_window,
            catalog,
            icon_service,
            deferred_icon_jobs: HashMap::new(),
            icon_requests: HashMap::new(),
            next_client_lifetime: 1,
            clients: HashMap::new(),
            managed_order: Vec::new(),
            frame_to_client: HashMap::new(),
            frame_extents_published: HashMap::new(),
            client_list_published: None,
            wallpaper_layout_published: None,
            redirect_frames: automatic_frame_redirect_available(conn, screen_num),
            registry: FrameRegistry::new(),
            sessions: HashMap::new(),
            renderer,
            animator,
            overlays: HashMap::new(),
            outline_overlays: HashMap::new(),
            outline_overlay_available,
            linger: animation::Linger::default(),
            damage_watch: HashMap::new(),
            minimized_captures: CaptureCache::default(),
            open_gate: OpenGate::default(),
            current_workspace: 0,
            workspace_count,
            window_config,
            active: None,
            workspace_focus: vec![None; workspace_count],
            screen_rect: Rect::new(
                0,
                0,
                u32::from(screen.width_in_pixels),
                u32::from(screen.height_in_pixels),
            ),
            output_work_areas: Vec::new(),
            applied_dock: None,
            controller_geometry: ControllerGeometryCache::default(),
            display_topology_dirty: TopologyDirty::default(),
            desktop_mode: desktop_mode::DesktopModeTracker::default(),
            raw_keys: None,
            show_desktop: show_desktop::ShowDesktop::default(),
            escape_pending: None,
            pings: Default::default(),
            focus_guard: Default::default(),
            stack_layers: HashMap::new(),
            switcher: switcher::Switcher::default(),
            kbd_grab: None,
            kbd_pending: None,
            mru: switcher::Mru::default(),
            switcher_signal: None,
            switcher_thumbs: Default::default(),
            switcher_previews: switcher::PreviewLedger::default(),
            switcher_damage: HashMap::new(),
            switcher_budget: switcher_thumb_provider::CaptureBudget::new(
                switcher::SWITCHER_CAPTURES_PER_TURN,
            ),
            dock_struts: HashMap::new(),
            snap_preview: SnapPreviewSurface::new(),
            chrome_runtimes: HashMap::new(),
            decoration_tokens: flamewm_skin::recipes::decoration::ChromeTokens::default(),
            titlebars_applied: crate::live_settings::titlebars(),
            window_margin_applied: crate::live_settings::window_margin(),
            rounded_clients: HashSet::new(),
            changes: WmChangeSet::default(),
            super_tap: SuperTap::default(),
            pointer_ingress_probe: PointerIngressProbe::default(),
            resize_probe: None,
            edge_drag: None,
            hot_corner: super::hot_corner::HotCornerState::new(),
            shake_locate: super::shake_locate::ShakeState::new(),
            mouse_workspace_cooldowns: MouseWorkspaceCooldowns::default(),
            chrome_deferred: None,
            chrome_paint_at: None,
            resize_configure_at: None,
            resize_deferred: None,
        })
    }
}
