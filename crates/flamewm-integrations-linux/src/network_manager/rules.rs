use super::*;

/// Interfaces whose `PropertiesChanged` the WM consumes. Device.Statistics (TxBytes/RxBytes)
/// is deliberately absent: NetworkManager emits it on a ~500 ms cadence and it drives no state.
pub(super) const NM_PROPERTY_INTERFACES: [&str; 3] = [
    "org.freedesktop.NetworkManager",
    "org.freedesktop.NetworkManager.Device",
    "org.freedesktop.NetworkManager.Device.Wireless",
];

/// Match rule for NetworkManager `PropertiesChanged` of one interface (arg0 = interface name),
/// so the bus only wakes the WM for interfaces it consumes.
pub(super) fn properties_changed_rule(interface: &str) -> String {
    format!(
        "type='signal',sender='org.freedesktop.NetworkManager',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',arg0='{interface}'"
    )
}

/// Refresh work implied by one wireless device `PropertiesChanged` signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DeviceRefresh {
    /// Re-read the device (ActiveConnection / ActiveAccessPoint); its reply re-queues the AP list.
    pub(super) device: bool,
    /// Re-read the device's access-point list (ActiveAccessPoint or LastScan changed).
    pub(super) access_points: bool,
}

/// Pure decision: the signal dict carries only changed properties, so a present value
/// that differs from the cached one is a change. Unchanged data requests nothing.
/// An undiscovered device (no cache) always requests a device read.
pub(super) fn device_signal_refresh(
    cached: Option<&WifiDevice>,
    signal: &NetworkManagerProperties,
) -> DeviceRefresh {
    let Some(cached) = cached else {
        return DeviceRefresh {
            device: true,
            access_points: false,
        };
    };
    let active_access_point = signal
        .active_access_point
        .as_deref()
        .map(|path| if path == "/" { "" } else { path });
    let access_point_changed =
        active_access_point.is_some_and(|path| path != cached.active_access_point);
    let connection_changed = signal
        .active_connection
        .as_deref()
        .is_some_and(|path| path != cached.active_connection);
    let scan_changed = signal
        .last_scan
        .is_some_and(|scan| Some(scan) != cached.last_scan);
    DeviceRefresh {
        device: access_point_changed || connection_changed,
        access_points: access_point_changed || scan_changed,
    }
}
