use flamewm_api::system::{NetworkSnapshot, ServiceAvailability, SystemAction, SystemSnapshot};
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

/// Project the filtered, connected-first AP list into the exact row slots
/// shown by the network surface. `slot` is the visible slot (not a provider
/// list index); `path` remains the stable NetworkManager object identity.
#[must_use]
pub fn project_visible_rows(
    snapshot: &SystemSnapshot,
    query: &flamewm_shell_core::status::NetworkQuery,
    scroll_offset: u32,
) -> Vec<NetworkRowView> {
    let network = &snapshot.network;
    let mut rows: Vec<_> = network
        .access_points
        .iter()
        .filter_map(|access_point| {
            let label = if access_point.ssid.is_empty() {
                "Hidden network".to_owned()
            } else {
                access_point.ssid.clone()
            };
            query.matches(&label).then(|| NetworkRowView {
                slot: 0,
                path: access_point.path.clone(),
                label,
                strength_percent: access_point.strength_percent.min(100),
                secured: access_point.secured,
                known: access_point.known,
                connected: is_connected_path(network, &access_point.path),
            })
        })
        .collect();
    rows.sort_by_key(|row| !row.connected);

    let (first, last) =
        flamewm_shell_core::status::visible_window(rows.len(), 44, 180, scroll_offset);
    rows.into_iter()
        .skip(first)
        .take(last.saturating_sub(first).min(NETWORK_SLOT_COUNT))
        .enumerate()
        .map(|(slot, mut row)| {
            row.slot = slot;
            row
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
    ceil_div(snapshot.access_points.len(), NETWORK_SLOT_COUNT)
}

/// True when `path` is the connected access-point object path. Keyed by AP
/// path (never SSID), so duplicate SSIDs yield exactly one connected row.
#[must_use]
pub fn is_connected_path(snapshot: &NetworkSnapshot, path: &str) -> bool {
    !snapshot.active_access_point_path.is_empty()
        && path == snapshot.active_access_point_path
}

/// The single connected row, for the dedicated `#network-connected` container.
/// It is independent of paging, so it stays visible on every page.
#[must_use]
pub fn connected_row(snapshot: &SystemSnapshot) -> Option<NetworkRowView> {
    let network = &snapshot.network;
    let access_point = network
        .access_points
        .iter()
        .find(|access_point| is_connected_path(network, &access_point.path))?;
    Some(NetworkRowView {
        slot: 0,
        path: access_point.path.clone(),
        label: if access_point.ssid.is_empty() {
            "Hidden network".to_owned()
        } else {
            access_point.ssid.clone()
        },
        strength_percent: access_point.strength_percent.min(100),
        secured: access_point.secured,
        known: access_point.known,
        connected: true,
    })
}

/// Page containing the connected AP when present, else the first page.
/// Keeps the connected row visible while paging the full model.
#[must_use]
pub fn active_page(snapshot: &NetworkSnapshot) -> usize {
    if snapshot.active_access_point_path.is_empty() {
        return 0;
    }
    snapshot
        .access_points
        .iter()
        .position(|access_point| is_connected_path(snapshot, &access_point.path))
        .map_or(0, |index| index / NETWORK_SLOT_COUNT)
}

/// `(slot, AP-path)` pairs for the active page in model order.
#[must_use]
pub fn paged_paths(snapshot: &NetworkSnapshot) -> Vec<(usize, String)> {
    paged_paths_for_page(snapshot, active_page(snapshot))
}

#[must_use]
pub fn paged_paths_for_page(snapshot: &NetworkSnapshot, page: usize) -> Vec<(usize, String)> {
    if snapshot.access_points.is_empty() {
        return Vec::new();
    }
    let pages = ceil_div(snapshot.access_points.len(), NETWORK_SLOT_COUNT);
    let clamped = page.min(pages.saturating_sub(1));
    snapshot
        .access_points
        .iter()
        .skip(clamped * NETWORK_SLOT_COUNT)
        .take(NETWORK_SLOT_COUNT)
        .enumerate()
        .map(|(slot, access_point)| (slot, access_point.path.clone()))
        .collect()
}

/// Snapshot AP count (full model rows, never the paged window).
#[must_use]
pub fn snapshot_ap_count(snapshot: &NetworkSnapshot) -> usize {
    snapshot.access_points.len()
}

/// Visible-row count for the active page (what projection maps into slots).
#[must_use]
pub fn project_visible_count(snapshot: &NetworkSnapshot) -> usize {
    paged_paths(snapshot).len()
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
    let visible_rows = network
        .access_points
        .iter()
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
        popover: popover.clone(),
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
    fn duplicate_ssids_yield_exactly_one_connected_row_keyed_by_path() {
        let snapshot = duplicate_ssid_snapshot("/nm/ap/2");
        let query = flamewm_shell_core::status::NetworkQuery::default();
        let rows = project_visible_rows(&snapshot, &query, 0);
        let connected: Vec<_> = rows.iter().filter(|row| row.connected).collect();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].path, "/nm/ap/2");
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
        // Connected AP is on page 2 (index 9); the connected container still resolves it.
        assert_eq!(active_page(&snapshot.network), 1);
        assert_eq!(connected_row(&snapshot).expect("connected").path, "/nm/ap/10");
        assert_eq!(page_count(&snapshot.network), 2);
    }

}
