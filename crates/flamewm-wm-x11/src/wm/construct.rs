//! Wm construction and the become-WM handshake.
use super::*;

pub(super) fn become_wm<C: Connection>(conn: &C, screen: &Screen) -> Result<(), ReplyError> {
    let attrs = ChangeWindowAttributesAux::new().event_mask(super::hot_corner::root_event_mask());
    let result = conn.change_window_attributes(screen.root, &attrs)?.check();
    conn.randr_select_input(
        screen.root,
        randr::NotifyMask::SCREEN_CHANGE
            | randr::NotifyMask::OUTPUT_CHANGE
            | randr::NotifyMask::CRTC_CHANGE,
    )?
    .check()?;
    if let Err(ReplyError::X11Error(ref error)) = result
        && error.error_kind == ErrorKind::Access
    {
        eprintln!("flamewm: another window manager is already running on this display");
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
        let renderer = flamewm_render_x11::ExternalDecorationRenderer::open("IBM Plex Sans")
            .ok()
            .map(flamewm_render_x11::ExternalDecorationRenderer::into_cursor_shared);
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
            mru: switcher::Mru::default(),
            switcher_signal: None,
            switcher_thumbs: Default::default(),
            switcher_previews: switcher::PreviewLedger::default(),
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
        })
    }
}
