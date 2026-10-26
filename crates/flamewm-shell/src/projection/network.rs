use super::*;

pub fn project_network_secret(
    document: &mut impl UiDocumentAccess,
    secret: &str,
) -> Result<(), String> {
    document.text("network-secret-value", "*".repeat(secret.chars().count()))
}

pub fn project_network(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    project_network_full(
        document,
        snapshot,
        &flamewm_shell_core::status::NetworkQuery::default(),
        0,
    )
}

/// Full network projection: Networks header, `WiFi` toggle, search filter over
/// the full snapshot, Connected/Available grouping, full AP scroll/
/// virtualization, secure Connect via `SecretAgent`. No fake hotspot/QR/
/// traffic. The secret value itself is never projected.
pub fn project_network_full(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    query: &flamewm_shell_core::status::NetworkQuery,
    scroll_offset: u32,
) -> Result<(), String> {
    // Rows keep model order and AP path identity; `popover_view` pages the AP list
    // and only the page window maps into the `network-slot-N` rows. The secret value
    // is never projected.
    let view = crate::taskbar::status::network::project(&snapshot.system);
    let popover = crate::taskbar::status::network::popover_view(&snapshot.system);
    // Dedicated connected container: independent of paging, shown on every page.
    // The connected row is filtered by the same search query as Available rows.
    let connected = crate::taskbar::status::network::connected_row(&snapshot.system)
        .filter(|row| query.matches(&row.label));
    document.text(
        "network-connected",
        connected
            .as_ref()
            .map_or_else(String::new, |row| row.label.clone()),
    )?;
    document.text(
        "network-connected-status",
        connected.as_ref().map_or_else(String::new, |row| {
            format!("Connected · {}%", row.strength_percent)
        }),
    )?;
    document.visible("network-connected-row", connected.is_some())?;
    document.visible("network-connected", connected.is_some())?;
    if let Some(popover) = popover {
        document.text("wifi-connected-name", popover.connected_label)?;
        // The label stays static; the query lives inside the input box so the
        // layout never shifts while typing.
        let filter_empty = query.filter.trim().is_empty();
        document.text(
            "network-filter-value",
            if filter_empty {
                String::new()
            } else {
                query.filter.clone()
            },
        )?;
        document.visible("network-filter-placeholder", filter_empty)?;
        document.visible("network-wifi-toggle", true)?;
        document.toggle(
            "network-wifi-track",
            "network-wifi-knob",
            snapshot.system.network.wifi_enabled,
        )?;
        if snapshot.system.network.wifi_enabled {
            // The shared toggle recipe paints a fixed skin red; the on-track follows the live accent.
            document.background("network-wifi-track", crate::projection::task_accent())?;
        }
        document.visible("network-scan", popover.can_scan)?;
        document.visible("network-disconnect", popover.can_disconnect)?;
        document.visible("network-secret", popover.secret_masked)?;
        let window = crate::taskbar::status::network::project_visible_rows(
            &snapshot.system,
            query,
            scroll_offset,
        );
        for slot in 0..crate::taskbar::status::network::NETWORK_SLOT_COUNT {
            let row = window.get(slot);
            let (mut name, mut status) = (String::new(), String::new());
            if let Some(row) = row {
                name.clone_from(&row.label);
                status = format!("{}%", row.strength_percent);
                if row.secured {
                    status.push_str(" · secured");
                }
                if row.known {
                    status.push_str(" · known");
                }
                if row.connected {
                    status.push_str(" · connected");
                }
                if popover.pending_secret_path.as_deref() == Some(row.path.as_str()) {
                    status.push_str(" · password required");
                }
            }
            document.visible(&format!("network-slot-{}", slot + 1), row.is_some())?;
            document.text(&format!("network-slot-{}-name", slot + 1), name)?;
            document.text(&format!("network-slot-{}-status", slot + 1), status)?;
        }
    } else {
        document.text("wifi-connected-name", view.label)?;
        for slot in 0..crate::taskbar::status::network::NETWORK_SLOT_COUNT {
            document.visible(&format!("network-slot-{}", slot + 1), false)?;
            document.text(&format!("network-slot-{}-name", slot + 1), String::new())?;
            document.text(&format!("network-slot-{}-status", slot + 1), String::new())?;
        }
        document.visible("network-wifi-toggle", false)?;
        document.visible("network-scan", false)?;
        document.visible("network-disconnect", false)?;
        document.visible("network-secret", false)?;
    }
    document.visible("wifi-connected-name", view.visible)?;
    Ok(())
}

#[cfg(test)]
mod network_template_tests {
    use super::*;
    use flamewm_api::system::ServiceAvailability;

    const NETWORK_ARTIFACT: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr"));

    fn network_document() -> flamewm_ui_x11::UiDocument {
        flamewm_ui_x11::decode_document(NETWORK_ARTIFACT).expect("network artifact must decode")
    }

    // CONTRACT-REGRESSION: every text target the network projection writes must be a
    // single-text-node element in the compiled network template.
    // TRIGGER: network.html id/child edits or project_network_full id changes.
    // OBSERVABLE: set_text on each projected id succeeds against the real artifact.
    // MUTATION: drop the text child of #network-connected; the open must be refused.
    #[test]
    fn every_projected_network_id_is_a_text_target() {
        let mut document = network_document();
        let mut ids = vec![
            "network-connected".to_owned(),
            "network-connected-status".to_owned(),
            "wifi-connected-name".to_owned(),
            "network-filter-value".to_owned(),
            "network-secret-value".to_owned(),
        ];
        for slot in 1..=8 {
            ids.push(format!("network-slot-{slot}-name"));
            ids.push(format!("network-slot-{slot}-status"));
        }
        for id in &ids {
            document
                .text(id, String::new())
                .unwrap_or_else(|error| panic!("network text target '{id}' refused: {error}"));
        }
    }

    // Forwards every write to the real decoded template while recording the
    // text and visibility the projection assigned to each id.
    struct NetworkRecorder<'a> {
        document: &'a mut flamewm_ui_x11::UiDocument,
        texts: std::collections::HashMap<String, String>,
        visible: std::collections::HashMap<String, bool>,
        toggles: std::collections::HashMap<String, bool>,
    }

    impl<'a> NetworkRecorder<'a> {
        fn new(document: &'a mut flamewm_ui_x11::UiDocument) -> Self {
            Self {
                document,
                texts: Default::default(),
                visible: Default::default(),
                toggles: Default::default(),
            }
        }
    }

    impl UiDocumentAccess for NetworkRecorder<'_> {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text.clone());
            UiDocumentAccess::text(self.document, id, text)
        }
        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.visible.insert(id.to_owned(), visible);
            UiDocumentAccess::visible(self.document, id, visible)
        }
        fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
            self.toggles.insert(track_id.to_owned(), checked);
            UiDocumentAccess::toggle(self.document, track_id, knob_id, checked)
        }
        forward!(
            image_rgb8(id: &str, image: RuntimeImage),
            image_rgba8(id: &str, image: RuntimeImage),
            position(id: &str, left: f32, top: f32),
            size(id: &str, width: f32, height: f32),
            background(id: &str, color: UiColor),
            border(id: &str, color: UiColor),
            foreground(id: &str, color: UiColor),
            background_clear(id: &str),
            border_clear(id: &str),
            foreground_clear(id: &str),
            font_size(id: &str, size_px: f32),
            font_weight(id: &str, weight: u16),
            overflow(id: &str, x: flamewm_ui_x11::Overflow, y: flamewm_ui_x11::Overflow),
            layer(id: &str, layer: flamewm_ui_core::style::UiLayer),
        );
    }

    // REGRESSION: with two access points (one connected), the real network
    // template must receive the connected row in #network-connected and the
    // first two Available slots must carry name text and visible=true.
    // OBSERVABLE: slot-1 name is the strongest AP, slot-2 the next; the
    // connected AP appears in the Connected container and the slot list.
    // REGRESSION: active AP path set but absent from scan must keep the
    // connected container visible with the active network label.
    #[test]
    fn absent_connected_access_point_keeps_connected_container_visible() {
        let mut document = network_document();
        let snapshot = network_snapshot(
            "/org/freedesktop/NetworkManager/AccessPoint/9",
            "LinsaFTW",
            vec![(
                "/org/freedesktop/NetworkManager/AccessPoint/2",
                "StubNeighbor",
                42,
            )],
        );
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert!(!recorder.texts["network-connected"].is_empty());
        assert_eq!(recorder.texts["network-connected"], "LinsaFTW");
        assert_eq!(recorder.visible.get("network-connected"), Some(&true));
    }

    #[test]
    fn projected_access_points_reach_slot_name_and_visible_flags() {
        let mut document = network_document();
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.network.availability = ServiceAvailability::Available;
        snapshot.system.network.wifi_enabled = true;
        snapshot.system.network.networking_enabled = true;
        snapshot.system.network.label = "LinsaFTW".to_owned();
        snapshot.system.network.active_access_point_path =
            "/org/freedesktop/NetworkManager/AccessPoint/1".to_owned();
        snapshot.system.network.access_points = vec![
            flamewm_api::system::NetworkAccessPointSnapshot {
                path: "/org/freedesktop/NetworkManager/AccessPoint/1".to_owned(),
                ssid: "LinsaFTW".to_owned(),
                strength_percent: 80,
                secured: true,
                known: true,
            },
            flamewm_api::system::NetworkAccessPointSnapshot {
                path: "/org/freedesktop/NetworkManager/AccessPoint/2".to_owned(),
                ssid: "StubNeighbor".to_owned(),
                strength_percent: 42,
                secured: false,
                known: false,
            },
        ];
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.texts["network-connected"], "LinsaFTW");
        assert_eq!(recorder.visible.get("network-connected"), Some(&true));
        // The connected AP lives only in the connected container; the Available
        // slots list the remaining APs without repeating it.
        assert_eq!(recorder.texts["network-slot-1-name"], "StubNeighbor");
        assert_eq!(recorder.visible.get("network-slot-1"), Some(&true));
        assert_eq!(recorder.texts["network-slot-1-status"], "42%");
        assert_eq!(recorder.visible.get("network-slot-2"), Some(&false));
        assert_eq!(recorder.texts["network-slot-2-name"], "");
    }

    fn network_snapshot(
        active_ap: &str,
        label: &str,
        access_points: Vec<(&str, &str, u8)>,
    ) -> crate::ShellSnapshot {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.network.availability = ServiceAvailability::Available;
        snapshot.system.network.wifi_enabled = true;
        snapshot.system.network.networking_enabled = true;
        snapshot.system.network.label = label.to_owned();
        snapshot.system.network.strength_percent = 72;
        snapshot.system.network.active_access_point_path = active_ap.to_owned();
        snapshot.system.network.access_points = access_points
            .into_iter()
            .map(
                |(path, ssid, strength)| flamewm_api::system::NetworkAccessPointSnapshot {
                    path: path.to_owned(),
                    ssid: ssid.to_owned(),
                    strength_percent: strength,
                    secured: true,
                    known: false,
                },
            )
            .collect();
        snapshot
    }

    // REGRESSION: the Connected row carries the status line "Connected · N%" from
    // the model strength only; no traffic rates are fabricated.
    #[test]
    fn connected_row_status_line_uses_model_strength_only() {
        let mut document = network_document();
        let snapshot = network_snapshot(
            "/ap/1",
            "LinsaFTW",
            vec![("/ap/1", "LinsaFTW", 72), ("/ap/2", "StubNeighbor", 42)],
        );
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(
            recorder.texts["network-connected-status"],
            "Connected · 72%"
        );
        assert_eq!(recorder.visible.get("network-connected-row"), Some(&true));
    }

    // REGRESSION: an active AP absent from the scan keeps the Connected row
    // visible, named from the network label, and never duplicated in Available.
    #[test]
    fn absent_connected_ap_keeps_connected_row_and_adds_no_available_duplicate() {
        let mut document = network_document();
        let snapshot = network_snapshot("/ap/9", "LinsaFTW", vec![("/ap/2", "StubNeighbor", 42)]);
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.texts["network-connected"], "LinsaFTW");
        assert_eq!(recorder.visible.get("network-connected-row"), Some(&true));
        assert_eq!(recorder.texts["network-slot-1-name"], "StubNeighbor");
        assert_eq!(recorder.visible.get("network-slot-2"), Some(&false));
    }

    // REGRESSION: duplicate SSIDs are keyed by AP path. The active path (/ap/2)
    // is the connected container only; the same-SSID /ap/1 stays an Available slot.
    #[test]
    fn duplicate_ssids_keep_the_non_active_access_point_in_available_slots() {
        let mut document = network_document();
        let snapshot = network_snapshot(
            "/ap/2",
            "FlameNet",
            vec![("/ap/1", "FlameNet", 80), ("/ap/2", "FlameNet", 50)],
        );
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.texts["network-connected"], "FlameNet");
        assert_eq!(recorder.texts["network-slot-1-name"], "FlameNet");
        assert_eq!(recorder.texts["network-slot-1-status"], "80% · secured");
        assert_eq!(recorder.visible.get("network-slot-2"), Some(&false));
        assert_eq!(recorder.texts["network-slot-2-name"], "");
    }

    // REGRESSION: the search query filters both groups. A non-matching
    // Connected row is hidden and only matching Available rows remain.
    #[test]
    fn search_filters_connected_and_available_groups() {
        let mut document = network_document();
        let snapshot = network_snapshot(
            "/ap/1",
            "LinsaFTW",
            vec![
                ("/ap/1", "LinsaFTW", 72),
                ("/ap/2", "StubNeighbor", 42),
                ("/ap/3", "OtherStub", 30),
            ],
        );
        let query = flamewm_shell_core::status::NetworkQuery {
            filter: "stub".to_owned(),
        };
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network_full(&mut recorder, &snapshot, &query, 0)
            .expect("network projection must succeed");
        assert_eq!(recorder.visible.get("network-connected-row"), Some(&false));
        assert_eq!(recorder.texts["network-slot-1-name"], "StubNeighbor");
        assert_eq!(recorder.texts["network-slot-2-name"], "OtherStub");
        assert_eq!(recorder.visible.get("network-slot-3"), Some(&false));
    }

    // REGRESSION: the query is projected into the input box value node, not the
    // static label. The label is never rewritten ("Filter: stub" would shift the
    // input), and the placeholder shows only while the query is empty.
    // MUTATION: a fix that writes "Filter: <query>" into the label, or leaves the
    // placeholder visible for a non-empty query, fails these assertions.
    #[test]
    fn filter_query_is_projected_inside_the_input_box() {
        let mut document = network_document();
        let snapshot = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/1", "LinsaFTW", 72)]);
        let mut recorder = NetworkRecorder::new(&mut document);
        let typed = flamewm_shell_core::status::NetworkQuery {
            filter: "stub".to_owned(),
        };
        project_network_full(&mut recorder, &snapshot, &typed, 0)
            .expect("network projection must succeed");
        assert_eq!(recorder.texts["network-filter-value"], "stub");
        assert_eq!(
            recorder.visible.get("network-filter-placeholder"),
            Some(&false)
        );
        assert!(!recorder.texts.contains_key("network-filter-label"));

        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.texts["network-filter-value"], "");
        assert_eq!(
            recorder.visible.get("network-filter-placeholder"),
            Some(&true)
        );
    }

    // REGRESSION: the Wi-Fi control is the canonical switch. Its track reflects
    // the real wifi_enabled state (on), with no optimistic local state.
    // MUTATION: a hard-coded or inverted toggle state fails this assertion.
    #[test]
    fn wifi_toggle_projects_enabled_state_as_on() {
        let mut document = network_document();
        let snapshot = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/1", "LinsaFTW", 72)]);
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.visible.get("network-wifi-toggle"), Some(&true));
        assert_eq!(recorder.toggles.get("network-wifi-track"), Some(&true));
    }

    // REGRESSION: wifi_enabled=false projects the switch as OFF, so a wrong fix that
    // always projects ON (or ignores the snapshot) is rejected.
    #[test]
    fn wifi_toggle_projects_disabled_state_as_off() {
        let mut document = network_document();
        let mut snapshot = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/1", "LinsaFTW", 72)]);
        snapshot.system.network.wifi_enabled = false;
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.visible.get("network-wifi-toggle"), Some(&true));
        assert_eq!(recorder.toggles.get("network-wifi-track"), Some(&false));
    }

    // REGRESSION: the Wi-Fi switch is a data-action button whose track and knob are
    // children. Hit-testing only resolves the button rect, so the track and knob must
    // lie inside the button, and the button must not cover the Connected row.
    // MUTATION: a button without a row layout (children stacked below its box) fails.
    #[test]
    fn wifi_switch_geometry_is_inside_button_and_clear_of_connected_row() {
        const POPUP: (f32, f32) = (461.0, 453.0);
        let mut document = network_document();
        let snapshot = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/1", "LinsaFTW", 72)]);
        {
            let mut recorder = NetworkRecorder::new(&mut document);
            project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        }
        let rect = |id: &str| {
            document
                .layout_box_by_id(id, POPUP)
                .unwrap_or_else(|| panic!("layout box for '{id}' missing"))
        };
        let button = rect("network-wifi-toggle");
        let track = rect("network-wifi-track");
        let knob = rect("network-wifi-knob");
        let connected = rect("network-connected-row");
        let contains = |outer: (f32, f32, f32, f32), inner: (f32, f32, f32, f32)| {
            inner.0 >= outer.0
                && inner.1 >= outer.1
                && inner.0 + inner.2 <= outer.0 + outer.2
                && inner.1 + inner.3 <= outer.1 + outer.3
        };
        let center = |r: (f32, f32, f32, f32)| (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        let inside = |outer: (f32, f32, f32, f32), point: (f32, f32)| {
            point.0 >= outer.0
                && point.0 < outer.0 + outer.2
                && point.1 >= outer.1
                && point.1 < outer.1 + outer.3
        };
        assert!(
            inside(button, center(track)),
            "track centre {track:?} outside button {button:?}"
        );
        assert!(
            inside(button, center(knob)),
            "knob centre {knob:?} outside button {button:?}"
        );
        assert!(
            contains(button, track),
            "track {track:?} not inside button {button:?}"
        );
        let intersects = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
        };
        assert!(
            !intersects(button, connected),
            "wifi button {button:?} overlaps connected row {connected:?}"
        );
    }
}
