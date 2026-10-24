use flamewm_api::system::{
    NetworkAccessPointSnapshot, NetworkSnapshot, ServiceAvailability, SystemAction, SystemSnapshot,
};
use flamewm_control_core::ControlRequest;
use flamewm_shell_core::{NetworkPopoverModel, NetworkView};

use super::super::intent::StatusIntent;

/// Maximum network rows a popover page can show. The compiled network
/// template exposes eight `network-slot-N` rows; paging selects which AP
/// model rows map into those slots. The model itself is never truncated.
pub const NETWORK_SLOT_COUNT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRowView {
    pub slot: usize,
    pub path: String,
    pub label: String,
    pub strength_percent: u8,
    pub secured: bool,
    pub known: bool,
    pub connected: bool,
}

/// Paged AP rows with stable AP path identity plus the wifi/scan/disconnect
/// state and masked secret prompt carried by the snapshot.
///
/// Rows come only from `snapshot.access_points` projected through the
/// existing [`NetworkPopoverModel`] ordering. No synthetic rows are
/// fabricated; paging selects a stable window and the full AP list is
/// preserved. Stable IDs are the AP `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPopoverView {
    pub popover: NetworkPopoverModel,
    pub connected_label: String,
    pub wifi_enabled: bool,
    pub can_scan: bool,
    pub can_disconnect: bool,
    pub active_page: usize,
    pub page_count: usize,
    pub visible_rows: Vec<NetworkRowView>,
    pub pending_secret_path: Option<String>,
    pub secret_masked: bool,
}

fn row_label(access_point: &NetworkAccessPointSnapshot) -> &str {
    if access_point.ssid.is_empty() {
        "Hidden network"
    } else {
        access_point.ssid.as_str()
    }
}

/// Project the filtered Available AP list (connected AP excluded) into the
/// exact row slots shown by the network surface. `slot` is the visible slot (not a provider
/// list index); `path` remains the stable NetworkManager object identity.
#[must_use]
pub fn project_visible_rows(
    snapshot: &SystemSnapshot,
    query: &flamewm_shell_core::status::NetworkQuery,
    scroll_offset: u32,
) -> Vec<NetworkRowView> {
    let network = &snapshot.network;
    // Filter by reference and clone only the visible window: the list can hold far more
    // access points than the slots shown.
    let needle = query.needle();
    let rows: Vec<&NetworkAccessPointSnapshot> = network
        .access_points
        .iter()
        .filter(|access_point| !is_connected_path(network, &access_point.path))
        .filter(|access_point| {
            flamewm_shell_core::status::NetworkQuery::matches_needle(&needle, row_label(access_point))
        })
        .collect();

    let (first, last) =
        flamewm_shell_core::status::visible_window(rows.len(), 44, 180, scroll_offset);
    rows.into_iter()
        .skip(first)
        .take(last.saturating_sub(first).min(NETWORK_SLOT_COUNT))
        .enumerate()
        .map(|(slot, access_point)| NetworkRowView {
            slot,
            path: access_point.path.clone(),
            label: row_label(access_point).to_owned(),
            strength_percent: access_point.strength_percent.min(100),
            secured: access_point.secured,
            known: access_point.known,
            connected: false,
        })
        .collect()
}

#[must_use]
pub fn project(snapshot: &SystemSnapshot) -> NetworkView {
    let mut view = flamewm_shell_core::StatusViews::from_snapshot(snapshot).network;
    // Availability matrix: Unknown visible but disabled, Available visible
    // and enabled, Unavailable hidden. Intent/popover stay Available-only.
    view.visible = snapshot.network.availability != ServiceAvailability::Unavailable;
    view.enabled = snapshot.network.availability == ServiceAvailability::Available;
    view
}

#[must_use]
pub fn page_count(snapshot: &NetworkSnapshot) -> usize {
    ceil_div(
        available_access_points(snapshot).count(),
        NETWORK_SLOT_COUNT,
    )
}

/// Access points shown in the Available slots: every scanned AP except the
/// connected one, which has its own `#network-connected` container. Keyed by
/// AP path, so a duplicate SSID of the connected AP stays listed.
pub fn available_access_points(
    snapshot: &NetworkSnapshot,
) -> impl Iterator<Item = &flamewm_api::system::NetworkAccessPointSnapshot> {
    snapshot
        .access_points
        .iter()
        .filter(move |access_point| !is_connected_path(snapshot, &access_point.path))
}

/// True when `path` is the connected access-point object path. Keyed by AP
/// path (never SSID), so duplicate SSIDs yield exactly one connected row.
#[must_use]
pub fn is_connected_path(snapshot: &NetworkSnapshot, path: &str) -> bool {
    !snapshot.active_access_point_path.is_empty() && path == snapshot.active_access_point_path
}

/// The single connected row, for the dedicated `#network-connected` container.
/// It is independent of paging, so it stays visible on every page. When the
/// active AP path is set but absent from the scan, the row is built from the
/// active path and the network label so the connection is never hidden.
#[must_use]
pub fn connected_row(snapshot: &SystemSnapshot) -> Option<NetworkRowView> {
    let network = &snapshot.network;
    if network.active_access_point_path.is_empty() {
        return None;
    }
    let label_for = |ssid: &str| {
        if ssid.is_empty() {
            "Hidden network".to_owned()
        } else {
            ssid.to_owned()
        }
    };
    let row = match network
        .access_points
        .iter()
        .find(|access_point| is_connected_path(network, &access_point.path))
    {
        Some(access_point) => NetworkRowView {
            slot: 0,
            path: access_point.path.clone(),
            label: label_for(&access_point.ssid),
            strength_percent: access_point.strength_percent.min(100),
            secured: access_point.secured,
            known: access_point.known,
            connected: true,
        },
        None => NetworkRowView {
            slot: 0,
            path: network.active_access_point_path.clone(),
            label: label_for(&network.label),
            strength_percent: network.strength_percent.min(100),
            secured: false,
            known: false,
            connected: true,
        },
    };
    Some(row)
}

/// Available slots always start on the first page; the connected AP is
/// never paged into the slot list.
#[must_use]
pub fn active_page(_snapshot: &NetworkSnapshot) -> usize {
    0
}

#[must_use]
pub fn paged_paths_for_page(snapshot: &NetworkSnapshot, page: usize) -> Vec<(usize, String)> {
    let available = available_access_points(snapshot).count();
    if available == 0 {
        return Vec::new();
    }
    let pages = ceil_div(available, NETWORK_SLOT_COUNT);
    let clamped = page.min(pages.saturating_sub(1));
    available_access_points(snapshot)
        .skip(clamped * NETWORK_SLOT_COUNT)
        .take(NETWORK_SLOT_COUNT)
        .enumerate()
        .map(|(slot, access_point)| (slot, access_point.path.clone()))
        .collect()
}

#[must_use]
pub fn popover(snapshot: &SystemSnapshot) -> Option<NetworkPopoverModel> {
    if snapshot.network.availability != ServiceAvailability::Available {
        return None;
    }
    flamewm_shell_core::StatusPopovers::from_snapshot(snapshot).network
}

#[must_use]
pub fn popover_view(snapshot: &SystemSnapshot) -> Option<NetworkPopoverView> {
    let popover = popover(snapshot)?;
    let network = &snapshot.network;
    let count = page_count(network);
    let page = active_page(network).min(count.saturating_sub(1));
    let visible_rows = available_access_points(network)
        .skip(page * NETWORK_SLOT_COUNT)
        .take(NETWORK_SLOT_COUNT)
        .enumerate()
        .map(|(slot, access_point)| NetworkRowView {
            slot,
            path: access_point.path.clone(),
            label: if access_point.ssid.is_empty() {
                "Hidden network".to_owned()
            } else {
                access_point.ssid.clone()
            },
            strength_percent: access_point.strength_percent.min(100),
            secured: access_point.secured,
            known: access_point.known,
            connected: is_connected_path(network, &access_point.path),
        })
        .collect();
    Some(NetworkPopoverView {
        connected_label: popover.details.clone(),
        wifi_enabled: popover.wifi_enabled,
        can_scan: popover.can_scan,
        can_disconnect: popover.can_disconnect,
        active_page: page,
        page_count: count,
        visible_rows,
        pending_secret_path: network
            .pending_secret
            .as_ref()
            .map(|request| request.access_point_path.clone()),
        secret_masked: network.pending_secret.is_some(),
        popover,
    })
}

/// Resolve a real AP path (or a scan/disconnect command) from this snapshot.
#[must_use]
pub fn system_action_for_path(
    snapshot: &SystemSnapshot,
    path: &str,
    scan: bool,
    disconnect: bool,
) -> Option<ControlRequest> {
    if snapshot.network.availability != ServiceAvailability::Available {
        return None;
    }
    let action = if scan {
        SystemAction::Scan
    } else if disconnect {
        if snapshot.network.active_path.is_empty() {
            return None;
        }
        SystemAction::Disconnect
    } else {
        let access_point = snapshot
            .network
            .access_points
            .iter()
            .find(|point| point.path == path)?;
        if access_point.known {
            SystemAction::ConnectKnown {
                access_point_path: access_point.path.clone(),
            }
        } else if access_point.secured {
            SystemAction::ConnectWifi {
                access_point_path: access_point.path.clone(),
                generation: snapshot.network.generation,
            }
        } else {
            // Unknown open AP: no `SystemAction::ConnectOpen` exists in the
            // shell-facing contract. Route through `ConnectKnown`; the
            // NetworkManager adapter detects the open+unknown AP and maps it
            // to `NetworkIntent::ConnectOpen`. `ConnectWifi` would be rejected
            // here (secure-new-network intent requires unknown secured AP).
            SystemAction::ConnectKnown {
                access_point_path: access_point.path.clone(),
            }
        }
    };
    Some(ControlRequest::SystemAction {
        action,
        expected_revision: snapshot.revision,
    })
}

#[must_use]
pub fn intent(snapshot: &SystemSnapshot) -> Option<StatusIntent> {
    (snapshot.network.availability == ServiceAvailability::Available)
        .then_some(StatusIntent::Network)
}

fn ceil_div(total: usize, per_page: usize) -> usize {
    if total == 0 {
        1
    } else {
        (total + per_page - 1) / per_page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::system::NetworkAccessPointSnapshot;

    #[test]
    fn availability_matrix_unknown_visible_disabled_available_enabled_unavailable_hidden() {
        let mut snapshot = SystemSnapshot::default();
        snapshot.network.availability = ServiceAvailability::Unknown;
        let view = project(&snapshot);
        assert!(view.visible, "Unknown stays visible");
        assert!(!view.enabled, "Unknown stays disabled");
        assert!(intent(&snapshot).is_none(), "intent stays Available-only");
        assert!(popover(&snapshot).is_none(), "popover stays Available-only");
        snapshot.network.availability = ServiceAvailability::Available;
        let view = project(&snapshot);
        assert!(view.visible && view.enabled);
        snapshot.network.availability = ServiceAvailability::Unavailable;
        let view = project(&snapshot);
        assert!(!view.visible, "Unavailable hidden");
        assert!(!view.enabled);
    }

    #[test]
    fn known_path_maps_to_revision_fenced_connect() {
        let mut snapshot = SystemSnapshot::default();
        snapshot.revision = 5;
        snapshot.network.availability = ServiceAvailability::Available;
        snapshot
            .network
            .access_points
            .push(NetworkAccessPointSnapshot {
                path: "/ap/1".to_owned(),
                ssid: "known".to_owned(),
                strength_percent: 80,
                secured: true,
                known: true,
            });
        assert_eq!(
            system_action_for_path(&snapshot, "/ap/1", false, false),
            Some(ControlRequest::SystemAction {
                action: SystemAction::ConnectKnown {
                    access_point_path: "/ap/1".to_owned()
                },
                expected_revision: 5
            })
        );
    }

    fn duplicate_ssid_snapshot(active_ap: &str) -> SystemSnapshot {
        let mut snapshot = SystemSnapshot::default();
        snapshot.network.availability = ServiceAvailability::Available;
        snapshot.network.active_path = "/nm/ac/1".to_owned();
        snapshot.network.active_access_point_path = active_ap.to_owned();
        snapshot.network.access_points = ["/nm/ap/1", "/nm/ap/2"]
            .iter()
            .map(|path| NetworkAccessPointSnapshot {
                path: (*path).to_owned(),
                ssid: "FlameNet".to_owned(),
                strength_percent: 60,
                secured: true,
                known: true,
            })
            .collect();
        snapshot
    }

    #[test]
    fn duplicate_ssids_yield_one_connected_row_and_keep_the_other_available() {
        let snapshot = duplicate_ssid_snapshot("/nm/ap/2");
        let query = flamewm_shell_core::status::NetworkQuery::default();
        let rows = project_visible_rows(&snapshot, &query, 0);
        // Keyed by path: the connected /nm/ap/2 is not an Available row, and the
        // same-SSID /nm/ap/1 is still listed as Available.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/nm/ap/1");
        assert!(rows.iter().all(|row| !row.connected));
        let header = connected_row(&snapshot).expect("connected row");
        assert_eq!(header.path, "/nm/ap/2");
        assert!(header.connected);
    }

    #[test]
    fn connection_path_alone_never_marks_an_access_point_connected() {
        // ActiveConnection ("/nm/ac/1") is not an AP path and must not match any row.
        let snapshot = duplicate_ssid_snapshot("");
        let query = flamewm_shell_core::status::NetworkQuery::default();
        assert!(project_visible_rows(&snapshot, &query, 0)
            .iter()
            .all(|row| !row.connected));
        assert!(connected_row(&snapshot).is_none());
        assert!(!is_connected_path(&snapshot.network, "/nm/ac/1"));
    }

    #[test]
    fn absent_connected_ap_still_yields_connected_row_from_active_path() {
        // Active AP path is set but the AP is missing from the scan: the connected
        // row must stay visible, built from the active path and the network label.
        let mut snapshot = duplicate_ssid_snapshot("/nm/ap/9");
        snapshot.network.label = "LinsaFTW".to_owned();
        snapshot.network.strength_percent = 70;
        let row = connected_row(&snapshot).expect("absent AP keeps connected row");
        assert_eq!(row.path, "/nm/ap/9");
        assert_eq!(row.label, "LinsaFTW");
        assert_eq!(row.strength_percent, 70);
        assert!(row.connected);
        let query = flamewm_shell_core::status::NetworkQuery::default();
        let rows = project_visible_rows(&snapshot, &query, 0);
        assert!(rows.iter().all(|row| row.path != "/nm/ap/9"));
    }

    #[test]
    fn absent_connected_ap_without_name_is_hidden_network() {
        let mut snapshot = duplicate_ssid_snapshot("/nm/ap/9");
        snapshot.network.label = String::new();
        let row = connected_row(&snapshot).expect("absent AP keeps connected row");
        assert_eq!(row.label, "Hidden network");
    }

    #[test]
    fn present_connected_ap_is_not_duplicated_by_fallback() {
        let snapshot = duplicate_ssid_snapshot("/nm/ap/2");
        let row = connected_row(&snapshot).expect("connected row");
        assert_eq!(row.path, "/nm/ap/2");
        assert_eq!(row.label, "FlameNet");
    }

    #[test]
    fn connected_row_is_available_on_every_page() {
        let mut snapshot = duplicate_ssid_snapshot("/nm/ap/10");
        snapshot.network.access_points = (1..=10)
            .map(|index| NetworkAccessPointSnapshot {
                path: format!("/nm/ap/{index}"),
                ssid: format!("Net{index}"),
                strength_percent: 50,
                secured: false,
                known: false,
            })
            .collect();
        // Connected AP is on page 2 (index 9) of the model, but it is excluded
        // from the Available slots: 9 remaining rows still fill 2 pages from page 0.
        assert_eq!(active_page(&snapshot.network), 0);
        assert_eq!(
            connected_row(&snapshot).expect("connected").path,
            "/nm/ap/10"
        );
        assert_eq!(page_count(&snapshot.network), 2);
        let query = flamewm_shell_core::status::NetworkQuery::default();
        assert!(project_visible_rows(&snapshot, &query, 0)
            .iter()
            .all(|row| row.path != "/nm/ap/10"));
    }

    #[test]
    fn connected_ap_is_excluded_from_available_slots_but_duplicate_ssid_stays() {
        let snapshot = duplicate_ssid_snapshot("/nm/ap/2");
        let query = flamewm_shell_core::status::NetworkQuery::default();
        let rows = project_visible_rows(&snapshot, &query, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/nm/ap/1");
        assert_eq!(rows[0].slot, 0);
        assert_eq!(rows[0].label, "FlameNet");
        assert!(!rows[0].connected);
        assert_eq!(
            connected_row(&snapshot).expect("connected").path,
            "/nm/ap/2"
        );
    }
}
