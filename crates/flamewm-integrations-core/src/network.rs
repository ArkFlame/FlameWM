use std::collections::BTreeSet;

use flamewm_api::system::{
    NetworkAccessPointSnapshot, NetworkKind, NetworkSecretRequestSnapshot, NetworkSnapshot,
    ServiceAvailability,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::backoff::next_reconnect_delay_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessPoint {
    pub path: String,
    pub ssid: String,
    pub strength_percent: u8,
    pub secured: bool,
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkStatus {
    pub snapshot: NetworkSnapshot,
    pub active_path: String,
    pub visible_access_points: Vec<AccessPoint>,
    pub networking_enabled: bool,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            snapshot: NetworkSnapshot::default(),
            active_path: String::new(),
            visible_access_points: Vec::new(),
            networking_enabled: false,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum NetworkIntent {
    SetWifiEnabled(bool),
    ConnectKnown {
        access_point_path: String,
    },
    RequestSecret {
        request_id: u64,
        access_point_path: String,
    },
    SubmitSecret {
        request_id: u64,
        secret: String,
    },
    CancelSecret {
        request_id: u64,
    },
    ConnectOpen {
        access_point_path: String,
    },
    Disconnect,
    Scan,
}

impl core::fmt::Debug for NetworkIntent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SetWifiEnabled(enabled) => {
                f.debug_tuple("SetWifiEnabled").field(enabled).finish()
            }
            Self::ConnectKnown { access_point_path } => f
                .debug_struct("ConnectKnown")
                .field("access_point_path", access_point_path)
                .finish(),
            Self::RequestSecret {
                request_id,
                access_point_path,
            } => f
                .debug_struct("RequestSecret")
                .field("request_id", request_id)
                .field("access_point_path", access_point_path)
                .finish(),
            Self::SubmitSecret { request_id, .. } => f
                .debug_struct("SubmitSecret")
                .field("request_id", request_id)
                .field("secret", &"<redacted>")
                .finish(),
            Self::CancelSecret { request_id } => f
                .debug_struct("CancelSecret")
                .field("request_id", request_id)
                .finish(),
            Self::ConnectOpen { access_point_path } => f
                .debug_struct("ConnectOpen")
                .field("access_point_path", access_point_path)
                .finish(),
            Self::Disconnect => f.write_str("Disconnect"),
            Self::Scan => f.write_str("Scan"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffConfig {
    pub initial_ms: u64,
    pub maximum_ms: u64,
    pub maximum_attempts: u8,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_ms: 250,
            maximum_ms: 8_000,
            maximum_attempts: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkController {
    generation: u64,
    status: NetworkStatus,
    available: bool,
    reconnect_attempts: u8,
    reconnect_pending: bool,
    backoff: BackoffConfig,
    ap_refresh_pending: bool,
    known_profiles: BTreeSet<String>,
    pending_secret: Option<NetworkSecretRequestSnapshot>,
    next_secret_request_id: u64,
}

impl Default for NetworkController {
    fn default() -> Self {
        Self {
            generation: 0,
            status: NetworkStatus::default(),
            available: false,
            reconnect_attempts: 0,
            reconnect_pending: false,
            backoff: BackoffConfig::default(),
            ap_refresh_pending: false,
            known_profiles: BTreeSet::new(),
            pending_secret: None,
            next_secret_request_id: 0,
        }
    }
}

impl NetworkController {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.available
    }

    #[must_use]
    pub fn status(&self) -> &NetworkStatus {
        &self.status
    }

    /// Produce the complete shell-facing snapshot without exposing NetworkManager adapter state.
    #[must_use]
    pub fn snapshot(&self) -> NetworkSnapshot {
        let mut snapshot = self.status.snapshot.clone();
        snapshot.generation = self.generation;
        snapshot.networking_enabled = self.status.networking_enabled;
        snapshot.active_path = self.status.active_path.clone();
        snapshot.access_points = self
            .status
            .visible_access_points
            .iter()
            .map(|ap| NetworkAccessPointSnapshot {
                path: ap.path.clone(),
                ssid: ap.ssid.clone(),
                strength_percent: ap.strength_percent.min(100),
                secured: ap.secured,
                known: ap.known,
            })
            .collect();
        snapshot.pending_secret = self.pending_secret.clone();
        snapshot
    }

    pub fn service_appeared(&mut self) {
        self.generation = self.generation.saturating_add(1).max(1);
        self.available = true;
        self.reconnect_attempts = 0;
        self.reconnect_pending = false;
        self.status.snapshot.availability = ServiceAvailability::Available;
        self.status.snapshot.generation = self.generation;
    }

    pub fn service_vanished(&mut self) {
        self.generation = self.generation.saturating_add(1).max(1);
        self.available = false;
        self.status = NetworkStatus {
            snapshot: NetworkSnapshot {
                availability: ServiceAvailability::Unavailable,
                generation: self.generation,
                kind: NetworkKind::Unavailable,
                ..NetworkSnapshot::default()
            },
            ..NetworkStatus::default()
        };
        self.known_profiles.clear();
        self.pending_secret = None;
        self.ap_refresh_pending = false;
        self.reconnect_pending = self.reconnect_attempts < self.backoff.maximum_attempts;
    }

    pub fn inject_status(&mut self, mut status: NetworkStatus, captured_generation: u64) -> bool {
        if captured_generation != self.generation {
            return false;
        }
        status.snapshot.generation = self.generation;
        status.snapshot.strength_percent = status.snapshot.strength_percent.min(100);
        self.known_profiles.clear();
        for ap in &mut status.visible_access_points {
            ap.strength_percent = ap.strength_percent.min(100);
            if ap.known {
                self.known_profiles.insert(ap.path.clone());
            }
        }
        self.status = status;
        true
    }

    pub fn request_set_wifi_enabled(&self, enabled: bool) -> FlameResult<NetworkIntent> {
        self.ensure_available()?;
        Ok(NetworkIntent::SetWifiEnabled(enabled))
    }

    pub fn request_connect_known(
        &self,
        access_point_path: &str,
        caller_generation: u64,
    ) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        if access_point_path.is_empty() || !self.known_profiles.contains(access_point_path) {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "saved NetworkManager access point was not found",
            ));
        }
        Ok(NetworkIntent::ConnectKnown {
            access_point_path: access_point_path.to_owned(),
        })
    }

    pub fn request_connect_wifi(
        &mut self,
        access_point_path: &str,
        caller_generation: u64,
    ) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        let Some(ap) = self
            .status
            .visible_access_points
            .iter()
            .find(|ap| ap.path == access_point_path)
        else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager access point was not found",
            ));
        };
        if !ap.secured || ap.known {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "secure-new-network intent requires an unknown secured access point",
            ));
        }
        Ok(NetworkIntent::RequestSecret {
            request_id: 0,
            access_point_path: access_point_path.to_owned(),
        })
    }

    /// Publish only metadata after NetworkManager asks its SecretAgent for credentials.
    pub fn request_secret_from_agent(
        &mut self,
        access_point_path: String,
    ) -> FlameResult<NetworkSecretRequestSnapshot> {
        self.ensure_available()?;
        if access_point_path.is_empty()
            || !self
                .status
                .visible_access_points
                .iter()
                .any(|access_point| access_point.path == access_point_path)
        {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager access point was not found",
            ));
        }
        self.next_secret_request_id = self.next_secret_request_id.saturating_add(1).max(1);
        let request = NetworkSecretRequestSnapshot {
            request_id: self.next_secret_request_id,
            access_point_path,
        };
        self.pending_secret = Some(request.clone());
        Ok(request)
    }

    pub fn request_submit_secret(
        &mut self,
        request_id: u64,
        caller_generation: u64,
        secret: String,
    ) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        self.validate_secret_request(request_id)?;
        self.pending_secret = None;
        Ok(NetworkIntent::SubmitSecret { request_id, secret })
    }

    pub fn request_cancel_secret(
        &mut self,
        request_id: u64,
        caller_generation: u64,
    ) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        self.validate_secret_request(request_id)?;
        self.pending_secret = None;
        Ok(NetworkIntent::CancelSecret { request_id })
    }

    pub fn cancel_secret_from_agent(&mut self) {
        self.pending_secret = None;
    }

    pub fn request_connect_open(
        &self,
        access_point_path: &str,
        caller_generation: u64,
    ) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        let Some(ap) = self
            .status
            .visible_access_points
            .iter()
            .find(|ap| ap.path == access_point_path)
        else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager access point was not found",
            ));
        };
        if ap.secured || ap.known {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "open-network intent requires an unknown open access point",
            ));
        }
        Ok(NetworkIntent::ConnectOpen {
            access_point_path: access_point_path.to_owned(),
        })
    }

    pub fn request_disconnect(&self, caller_generation: u64) -> FlameResult<NetworkIntent> {
        self.validate_generation(caller_generation)?;
        self.ensure_available()?;
        Ok(NetworkIntent::Disconnect)
    }

    pub fn request_scan(&mut self) -> FlameResult<Option<NetworkIntent>> {
        self.ensure_available()?;
        if self.ap_refresh_pending {
            return Ok(None);
        }
        self.ap_refresh_pending = true;
        Ok(Some(NetworkIntent::Scan))
    }

    pub fn complete_scan(&mut self, captured_generation: u64) -> bool {
        if captured_generation != self.generation {
            return false;
        }
        self.ap_refresh_pending = false;
        true
    }

    pub fn next_reconnect_delay_ms(&mut self) -> Option<u64> {
        next_reconnect_delay_ms(
            self.backoff.initial_ms,
            self.backoff.maximum_ms,
            self.backoff.maximum_attempts,
            &mut self.reconnect_attempts,
            &mut self.reconnect_pending,
        )
    }

    fn validate_generation(&self, caller_generation: u64) -> FlameResult<()> {
        if caller_generation != self.generation {
            Err(FlameError::new(
                ErrorCode::StaleRevision,
                "stale NetworkManager service generation",
            ))
        } else {
            Ok(())
        }
    }

    fn ensure_available(&self) -> FlameResult<()> {
        if self.available {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::Unavailable,
                "NetworkManager is unavailable",
            ))
        }
    }

    fn validate_secret_request(&self, request_id: u64) -> FlameResult<()> {
        if self
            .pending_secret
            .as_ref()
            .is_some_and(|request| request.request_id == request_id)
        {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager secret request was not found",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_loss_clears_stale_paths_and_invalidates_callbacks() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        controller.service_vanished();
        assert!(!controller.inject_status(NetworkStatus::default(), generation));
    }

    #[test]
    fn protected_network_intent_never_contains_secret() {
        let intent = NetworkIntent::SubmitSecret {
            request_id: 1,
            secret: "not-for-logs".to_owned(),
        };
        assert!(!format!("{intent:?}").contains("not-for-logs"));
    }

    #[test]
    fn secret_metadata_is_published_only_after_agent_request() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        let _ = controller.inject_status(
            NetworkStatus {
                visible_access_points: vec![AccessPoint {
                    path: "/ap/secure".to_owned(),
                    ssid: "Secure".to_owned(),
                    strength_percent: 80,
                    secured: true,
                    known: false,
                }],
                ..NetworkStatus::default()
            },
            generation,
        );
        let _ = controller
            .request_connect_wifi("/ap/secure", generation)
            .expect("secure access point accepted");
        assert_eq!(controller.snapshot().pending_secret, None);
        assert_eq!(
            controller
                .request_secret_from_agent("/ap/secure".to_owned())
                .expect("agent request accepted"),
            NetworkSecretRequestSnapshot {
                request_id: 1,
                access_point_path: "/ap/secure".to_owned(),
            }
        );
    }

    #[test]
    fn shell_snapshot_carries_visible_access_points_without_secrets() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        let _ = controller.inject_status(
            NetworkStatus {
                snapshot: NetworkSnapshot {
                    wifi_enabled: true,
                    ..NetworkSnapshot::default()
                },
                active_path: "/ap/1".to_owned(),
                visible_access_points: vec![AccessPoint {
                    path: "/ap/1".to_owned(),
                    ssid: "FlameNet".to_owned(),
                    strength_percent: 140,
                    secured: true,
                    known: true,
                }],
                networking_enabled: true,
            },
            generation,
        );
        assert_eq!(controller.snapshot().access_points[0].strength_percent, 100);
    }

    #[test]
    fn reconnect_attempts_are_bounded() {
        let mut controller = NetworkController::default();
        controller.service_vanished();
        let mut attempts = 0;
        while controller.next_reconnect_delay_ms().is_some() {
            attempts += 1;
        }
        assert_eq!(attempts, 8);
    }

    #[test]
    fn open_network_intent_accepts_only_unknown_open_access_points() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        let _ = controller.inject_status(
            NetworkStatus {
                visible_access_points: vec![AccessPoint {
                    path: "/ap/open".to_owned(),
                    ssid: "Open".to_owned(),
                    strength_percent: 50,
                    secured: false,
                    known: false,
                }],
                ..NetworkStatus::default()
            },
            generation,
        );
        assert_eq!(
            controller.request_connect_open("/ap/open", generation),
            Ok(NetworkIntent::ConnectOpen {
                access_point_path: "/ap/open".to_owned(),
            })
        );
    }

    #[test]
    fn wrong_secret_request_id_leaves_pending_secret_for_retry() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        let _ = controller.inject_status(
            NetworkStatus {
                visible_access_points: vec![AccessPoint {
                    path: "/ap/secure".to_owned(),
                    ssid: "Secure".to_owned(),
                    strength_percent: 80,
                    secured: true,
                    known: false,
                }],
                ..NetworkStatus::default()
            },
            generation,
        );
        let _ = controller
            .request_connect_wifi("/ap/secure", generation)
            .expect("secure access point accepted");
        let request = controller
            .request_secret_from_agent("/ap/secure".to_owned())
            .expect("agent request accepted");
        assert!(
            controller
                .request_submit_secret(request.request_id + 1, generation, "wrong".to_owned())
                .is_err()
        );
        assert_eq!(controller.snapshot().pending_secret, Some(request.clone()));
        assert_eq!(
            controller.request_submit_secret(request.request_id, generation, "right".to_owned()),
            Ok(NetworkIntent::SubmitSecret {
                request_id: request.request_id,
                secret: "right".to_owned(),
            })
        );
        assert_eq!(controller.snapshot().pending_secret, None);
    }

    #[test]
    fn scan_error_completion_clears_pending_scan() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let generation = controller.generation();
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(NetworkIntent::Scan))
        ));
        assert!(
            matches!(controller.request_scan(), Ok(None)),
            "scan is pending"
        );
        // The adapter calls complete_scan on a Scan reply error.
        assert!(controller.complete_scan(generation));
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(NetworkIntent::Scan))
        ));
    }

    #[test]
    fn stale_scan_completion_does_not_clear_new_generation_scan() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        let old_generation = controller.generation();
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(NetworkIntent::Scan))
        ));

        controller.service_vanished();
        controller.service_appeared();
        let new_generation = controller.generation();
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(NetworkIntent::Scan))
        ));

        assert!(!controller.complete_scan(old_generation));
        assert!(matches!(controller.request_scan(), Ok(None)));
        assert!(controller.complete_scan(new_generation));
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(NetworkIntent::Scan))
        ));
    }
}
