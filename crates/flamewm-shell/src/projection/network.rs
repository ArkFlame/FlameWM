use super::*;

pub fn project_network_secret(
    document: &mut impl UiDocumentAccess,
    secret: &str,
) -> Result<(), String> {
    document.text("network-secret-value", "*".repeat(secret.chars().count()))
}

/// The truthful connection sentence for the popup subtitle. The template carries no fixed prefix,
/// so every state reads as a full phrase ("Connected to X", "Not connected", ...).
#[must_use]
pub fn connection_sentence(system: &flamewm_api::system::SystemSnapshot) -> String {
    let network = &system.network;
    if !network.wifi_enabled && network.kind != flamewm_api::system::NetworkKind::Wired {
        return "Wi-Fi is off".to_owned();
    }
    match network.kind {
        flamewm_api::system::NetworkKind::Connecting => "Connecting".to_owned(),
        flamewm_api::system::NetworkKind::Wired if !network.active_path.is_empty() => {
            "Connected (wired)".to_owned()
        }
        flamewm_api::system::NetworkKind::Disconnected
        | flamewm_api::system::NetworkKind::Unavailable => "Not connected".to_owned(),
        _ => crate::taskbar::status::network::connected_row(system).map_or_else(
            || "Not connected".to_owned(),
            |row| format!("Connected to {}", row.label),
        ),
    }
}

/// Keyboard focus ring: the focused network row gets the live accent outline; the others clear it.
pub fn project_network_focus(
    document: &mut impl UiDocumentAccess,
    focus: Option<usize>,
) -> Result<(), String> {
    for slot in 0..crate::taskbar::status::network::NETWORK_SLOT_COUNT {
        let row = format!("network-slot-{}", slot + 1);
        if focus == Some(slot) {
            document.border(&row, crate::projection::task_accent())?;
        } else {
            document.border_clear(&row)?;
        }
    }
    Ok(())
}

/// Shows a failed or pending popup network action as one status line. `None` clears it.
pub fn project_network_status(
    document: &mut impl UiDocumentAccess,
    error: Option<&str>,
) -> Result<(), String> {
    document.text("network-action-error", error.unwrap_or_default().to_owned())
}

/// Expands the `>` details of the visible row whose AP path is `expanded`: its status line lists
/// the real model fields (signal, security, saved state, AP object). Rows not on this page are
/// left as projected. Only model values are shown; nothing is fabricated.
pub fn project_network_details(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    query: &flamewm_shell_core::status::NetworkQuery,
    scroll_offset: u32,
    expanded: Option<&str>,
) -> Result<(), String> {
    let Some(path) = expanded else {
        return Ok(());
    };
    let rows = crate::taskbar::status::network::project_visible_rows(
        &snapshot.system,
        query,
        scroll_offset,
    );
    let Some(row) = rows.iter().find(|row| row.path == path) else {
        return Ok(());
    };
    let access_point = row.path.rsplit('/').next().unwrap_or_default();
    let details = format!(
        "Signal {}% · {} · {} · AP {}",
        row.strength_percent,
        if row.secured { "secured" } else { "open" },
        if row.known {
            "saved profile"
        } else {
            "not saved"
        },
        access_point,
    );
    document.text(&format!("network-slot-{}-status", row.slot + 1), details)
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
    document.text_ref(
        "network-connected",
        connected.as_ref().map_or("", |row| row.label.as_str()),
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
        document.text("wifi-connected-name", connection_sentence(&snapshot.system))?;
        // The label stays static; the query lives inside the input box so the
        // layout never shifts while typing.
        let filter_empty = query.filter.trim().is_empty();
        document.text_ref(
            "network-filter-value",
            if filter_empty {
                ""
            } else {
                query.filter.as_str()
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
        // The connected tile follows the live accent; the stylesheet keeps no fixed blue.
        document.background("network-connected-tile", crate::projection::task_accent())?;
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
        document.text("wifi-connected-name", connection_sentence(&snapshot.system))?;
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
            "network-action-error".to_owned(),
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

    // REGRESSION: the subtitle is a full sentence per state; the template has no fixed "Connected to"
    // prefix, so a disconnected label can never read "Connected to Not connected".
    // MUTATION: restoring the static prefix or writing the raw label "Not connected" fails below.
    #[test]
    fn connection_subtitle_is_truthful_for_each_state() {
        let mut document = network_document();
        let connected = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/1", "LinsaFTW", 72)]);
        let mut recorder = NetworkRecorder::new(&mut document);
        project_network(&mut recorder, &connected).expect("projection must succeed");
        assert_eq!(
            recorder.texts["wifi-connected-name"],
            "Connected to LinsaFTW"
        );

        let mut disconnected =
            network_snapshot("", "Not connected", vec![("/ap/2", "Neighbor", 40)]);
        disconnected.system.network.kind = flamewm_api::system::NetworkKind::Disconnected;
        project_network(&mut recorder, &disconnected).expect("projection must succeed");
        assert_eq!(recorder.texts["wifi-connected-name"], "Not connected");

        let mut off = network_snapshot("", "", vec![]);
        off.system.network.wifi_enabled = false;
        project_network(&mut recorder, &off).expect("projection must succeed");
        assert_eq!(recorder.texts["wifi-connected-name"], "Wi-Fi is off");

        let mut connecting = network_snapshot("", "", vec![]);
        connecting.system.network.kind = flamewm_api::system::NetworkKind::Connecting;
        project_network(&mut recorder, &connecting).expect("projection must succeed");
        assert_eq!(recorder.texts["wifi-connected-name"], "Connecting");
    }

    // REGRESSION: ">" on an Available row expands that row's details from model fields only.
    // MUTATION: expanding the wrong slot or inventing a value fails the assertions.
    #[test]
    fn expanded_row_status_lists_model_details_for_that_slot() {
        let mut document = network_document();
        let snapshot = network_snapshot(
            "/ap/1",
            "LinsaFTW",
            vec![("/ap/1", "LinsaFTW", 72), ("/ap/2", "StubNeighbor", 42)],
        );
        let mut recorder = NetworkRecorder::new(&mut document);
        let snapshot_ref = &snapshot;
        super::project_network_details(
            &mut recorder,
            snapshot_ref,
            &flamewm_shell_core::status::NetworkQuery::default(),
            0,
            Some("/ap/2"),
        )
        .expect("details projection must succeed");
        assert_eq!(
            recorder.texts["network-slot-1-status"],
            "Signal 42% · secured · not saved · AP 2"
        );
        assert!(!recorder.texts.contains_key("network-slot-2-status"));
    }

    // FAILURE PATH: a details request for an AP that is not on this page changes nothing.
    #[test]
    fn details_for_unknown_path_projects_nothing() {
        let mut document = network_document();
        let snapshot = network_snapshot("/ap/1", "LinsaFTW", vec![("/ap/2", "StubNeighbor", 42)]);
        let mut recorder = NetworkRecorder::new(&mut document);
        super::project_network_details(
            &mut recorder,
            &snapshot,
            &flamewm_shell_core::status::NetworkQuery::default(),
            0,
            Some("/ap/404"),
        )
        .expect("details projection must succeed");
        assert!(recorder.texts.is_empty());
    }

    // FAILURE PATH: a failed popup action is shown as text, and a later success clears it.
    #[test]
    fn action_error_is_projected_then_cleared() {
        let mut document = network_document();
        let mut recorder = NetworkRecorder::new(&mut document);
        super::project_network_status(&mut recorder, Some("Scan failed: NetworkManager busy"))
            .expect("status projection must succeed");
        assert_eq!(
            recorder.texts["network-action-error"],
            "Scan failed: NetworkManager busy"
        );
        super::project_network_status(&mut recorder, None).expect("status projection must succeed");
        assert_eq!(recorder.texts["network-action-error"], "");
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
        snapshot.system.network.kind = flamewm_api::system::NetworkKind::Wireless;
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

/// Names for the Bluetooth row status: alias when present, address otherwise.
fn bluetooth_device_names(devices: Vec<&flamewm_api::system::BluetoothDevice>) -> String {
    devices
        .into_iter()
        .map(|device| {
            if device.name.is_empty() {
                device.address.as_str()
            } else {
                device.name.as_str()
            }
        })
        .collect::<Vec<&str>>()
        .join(", ")
}

/// Status line for the Bluetooth row in the network popup. `None` hides the row when no adapter
/// exists. Connected devices lead, then paired devices.
#[must_use]
pub fn bluetooth_row_status(bluetooth: &flamewm_api::system::BluetoothSnapshot) -> Option<String> {
    if !bluetooth.available {
        return None;
    }
    if !bluetooth.powered {
        return Some("Off".to_owned());
    }
    let connected = bluetooth_device_names(
        bluetooth
            .devices
            .iter()
            .filter(|device| device.connected)
            .collect(),
    );
    if !connected.is_empty() {
        return Some(format!("Connected: {connected}"));
    }
    let paired = bluetooth_device_names(
        bluetooth
            .devices
            .iter()
            .filter(|device| device.paired)
            .collect(),
    );
    if !paired.is_empty() {
        return Some(format!("Paired: {paired}"));
    }
    Some("On, no paired devices".to_owned())
}

/// The device a click on the Bluetooth row acts on: first connected, else first paired, else first.
#[must_use]
pub fn bluetooth_primary_device(
    bluetooth: &flamewm_api::system::BluetoothSnapshot,
) -> Option<&flamewm_api::system::BluetoothDevice> {
    bluetooth
        .devices
        .iter()
        .find(|device| device.connected)
        .or_else(|| bluetooth.devices.iter().find(|device| device.paired))
        .or_else(|| bluetooth.devices.first())
}

/// Typed trust toggle for the primary device, built through the validated device action. `None`
/// while the adapter is absent or powered off, or when no device is known: the click does nothing.
#[must_use]
pub fn bluetooth_trust_action(
    bluetooth: &flamewm_api::system::BluetoothSnapshot,
) -> Option<flamewm_api::system::SystemAction> {
    if !bluetooth.available || !bluetooth.powered {
        return None;
    }
    let device = bluetooth_primary_device(bluetooth)?;
    let action = flamewm_api::system::BluetoothDeviceAction::new(
        flamewm_api::system::BluetoothDeviceOp::SetTrusted(!device.trusted),
        &device.address,
    )?;
    Some(flamewm_api::system::SystemAction::BluetoothDevice(action))
}

/// VPN rows the network popup shows. Profiles past this count stay out of the popup.
pub const VPN_ROW_SLOTS: usize = 4;

/// Click on VPN row `slot` (1-based): activates an inactive profile, deactivates an active one.
/// `None` when the slot has no profile or the UUID is malformed.
#[must_use]
pub fn vpn_toggle_action(
    network: &flamewm_api::system::NetworkSnapshot,
    slot: usize,
) -> Option<flamewm_api::system::SystemAction> {
    let profile = network.vpn_profiles.get(slot.checked_sub(1)?)?;
    let uuid = flamewm_api::system::VpnConnectionUuid::new(&profile.uuid)?;
    let op = if profile.active {
        flamewm_api::system::VpnConnectionOp::Deactivate
    } else {
        flamewm_api::system::VpnConnectionOp::Activate
    };
    Some(flamewm_api::system::SystemAction::VpnConnection(
        flamewm_api::system::VpnConnectionAction { op, uuid },
    ))
}

/// Projects the VPN rows: name, and connected or not connected. Unused slots are hidden.
pub fn project_vpn_rows(
    document: &mut impl UiDocumentAccess,
    network: &flamewm_api::system::NetworkSnapshot,
) -> Result<(), String> {
    for slot in 1..=VPN_ROW_SLOTS {
        let profile = network.vpn_profiles.get(slot - 1);
        document.visible(&format!("network-vpn-row-{slot}"), profile.is_some())?;
        document.text(
            &format!("network-vpn-name-{slot}"),
            profile.map(|p| p.id.clone()).unwrap_or_default(),
        )?;
        document.text(
            &format!("network-vpn-status-{slot}"),
            profile
                .map(|p| {
                    if p.active {
                        "Connected"
                    } else {
                        "Not connected"
                    }
                })
                .unwrap_or_default()
                .to_owned(),
        )?;
    }
    Ok(())
}

pub fn project_bluetooth_row(
    document: &mut impl UiDocumentAccess,
    bluetooth: &flamewm_api::system::BluetoothSnapshot,
) -> Result<(), String> {
    let status = bluetooth_row_status(bluetooth);
    document.visible("network-bluetooth-row", status.is_some())?;
    document.text("network-bluetooth-status", status.unwrap_or_default())
}

#[cfg(test)]
mod vpn_row_tests {
    use super::*;
    use flamewm_api::system::{NetworkSnapshot, SystemAction, VpnConnectionOp, VpnProfileSnapshot};

    fn network(active: bool) -> NetworkSnapshot {
        NetworkSnapshot {
            vpn_profiles: vec![VpnProfileSnapshot {
                uuid: "3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f".to_owned(),
                id: "Office".to_owned(),
                active,
            }],
            ..NetworkSnapshot::default()
        }
    }

    #[test]
    fn vpn_row_click_activates_inactive_and_deactivates_active_profile() {
        let op = |active| match vpn_toggle_action(&network(active), 1) {
            Some(SystemAction::VpnConnection(action)) => Some(action.op),
            _ => None,
        };
        assert_eq!(op(false), Some(VpnConnectionOp::Activate));
        assert_eq!(op(true), Some(VpnConnectionOp::Deactivate));
    }

    #[test]
    fn vpn_row_click_ignores_empty_slot_and_malformed_uuid() {
        assert_eq!(vpn_toggle_action(&network(false), 2), None);
        assert_eq!(vpn_toggle_action(&network(false), 0), None);
        let mut bad = network(false);
        bad.vpn_profiles[0].uuid = "not-a-uuid".to_owned();
        assert_eq!(vpn_toggle_action(&bad, 1), None);
    }
}

#[cfg(test)]
mod bluetooth_row_tests {
    use super::*;
    use flamewm_api::system::{BluetoothDevice, BluetoothSnapshot, SystemAction};

    fn device(
        address: &str,
        name: &str,
        paired: bool,
        trusted: bool,
        connected: bool,
    ) -> BluetoothDevice {
        BluetoothDevice {
            path: format!("/org/bluez/hci0/dev_{}", address.replace(':', "_")),
            address: address.to_owned(),
            name: name.to_owned(),
            paired,
            trusted,
            connected,
        }
    }

    fn snapshot(powered: bool, devices: Vec<BluetoothDevice>) -> BluetoothSnapshot {
        BluetoothSnapshot {
            available: true,
            powered,
            devices,
        }
    }

    // REGRESSION: no adapter hides the row and offers no action.
    // MUTATION: ignoring `available` shows "Off" on machines without BlueZ.
    #[test]
    fn absent_adapter_hides_row_and_offers_no_action() {
        let bluetooth = BluetoothSnapshot::default();
        assert_eq!(bluetooth_row_status(&bluetooth), None);
        assert_eq!(bluetooth_trust_action(&bluetooth), None);
    }

    #[test]
    fn powered_off_adapter_reads_off_and_offers_no_action() {
        let bluetooth = snapshot(
            false,
            vec![device("AA:BB:CC:DD:EE:01", "Headset", true, true, true)],
        );
        assert_eq!(bluetooth_row_status(&bluetooth).as_deref(), Some("Off"));
        assert_eq!(bluetooth_trust_action(&bluetooth), None);
    }

    // REGRESSION: connected names lead the status; paired-only devices are a fallback.
    // MUTATION: listing paired before connected, or ignoring the connected flag, fails here.
    #[test]
    fn connected_devices_lead_then_paired_devices() {
        let connected = snapshot(
            true,
            vec![
                device("AA:BB:CC:DD:EE:02", "Mouse", true, false, false),
                device("AA:BB:CC:DD:EE:01", "Headset", true, false, true),
            ],
        );
        assert_eq!(
            bluetooth_row_status(&connected).as_deref(),
            Some("Connected: Headset")
        );

        let paired = snapshot(
            true,
            vec![device("AA:BB:CC:DD:EE:02", "Mouse", true, false, false)],
        );
        assert_eq!(
            bluetooth_row_status(&paired).as_deref(),
            Some("Paired: Mouse")
        );

        let bare = snapshot(true, Vec::new());
        assert_eq!(
            bluetooth_row_status(&bare).as_deref(),
            Some("On, no paired devices")
        );
    }

    #[test]
    fn empty_alias_falls_back_to_address() {
        let bluetooth = snapshot(
            true,
            vec![device("AA:BB:CC:DD:EE:03", "", true, false, true)],
        );
        assert_eq!(
            bluetooth_row_status(&bluetooth).as_deref(),
            Some("Connected: AA:BB:CC:DD:EE:03")
        );
    }

    // REGRESSION: the click targets the primary device and inverts its current trust.
    // MUTATION: sending the current trust instead of its negation, or the wrong device, fails here.
    #[test]
    fn click_toggles_trust_of_the_primary_device() {
        let bluetooth = snapshot(
            true,
            vec![
                device("AA:BB:CC:DD:EE:02", "Mouse", true, false, false),
                device("AA:BB:CC:DD:EE:01", "Headset", true, true, true),
            ],
        );
        let action = bluetooth_trust_action(&bluetooth).expect("trust action");
        let SystemAction::BluetoothDevice(action) = action else {
            panic!("expected a bluetooth device action");
        };
        assert_eq!(action.address, "AA:BB:CC:DD:EE:01");
        assert_eq!(
            action.op,
            flamewm_api::system::BluetoothDeviceOp::SetTrusted(false)
        );
    }

    #[test]
    fn click_without_devices_or_with_malformed_address_does_nothing() {
        assert_eq!(bluetooth_trust_action(&snapshot(true, Vec::new())), None);
        let bad = snapshot(true, vec![device("not-an-address", "X", true, false, true)]);
        assert_eq!(bluetooth_trust_action(&bad), None);
    }
}
