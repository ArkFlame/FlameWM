//! Proof seam (`FLAMEWM_SETTINGS_FIXTURE_OUTPUTS`): a JSON file supplies a fake multi-output
//! display snapshot and records display topology requests to a JSON log, so the Displays canvas
//! can be driven on a private `Xvfb` with a single `RandR` output. Every other request still reaches
//! the live control transport. The seam is inert unless the variable is set.
//!
//! Fixture shape:
//! `{"log": "/path/requests.json", "outputs": [{"connector": "DP-1", "x": 0, "y": 0,
//!   "width": 1920, "height": 1080, "primary": true}, ...]}`

use std::path::PathBuf;

use flamewm_api::display::{
    DisplayMode, DisplaySnapshot, OutputPlacement, OutputSnapshot, Rotation,
};
use flamewm_api::{ModeId, OutputId, Rect, Size, TransactionId};
use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_settings_core::ControlTransport;
use serde_json::{Value, json};

use crate::control::SettingsControl;

/// Environment variable naming the fixture JSON. Unset means the live control bus only.
pub const FIXTURE_ENV: &str = "FLAMEWM_SETTINGS_FIXTURE_OUTPUTS";

/// The transport the Settings window talks through. `Fixture` answers display requests from the
/// fixture and forwards everything else to the live bus connection.
pub enum SettingsTransport {
    Live(SettingsControl),
    Fixture {
        live: SettingsControl,
        displays: FixtureDisplays,
    },
}

impl ControlTransport for SettingsTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        match self {
            Self::Live(live) => live.call(request),
            Self::Fixture { live, displays } => match displays.answer(&request) {
                Some(response) => Ok(response),
                None => live.call(request),
            },
        }
    }
}

/// Connects the live bus and, when the fixture variable is set, wraps it in the display fixture.
pub fn connect() -> Result<SettingsTransport, String> {
    let live = SettingsControl::connect_session().map_err(|error| error.to_string())?;
    let Some(path) = std::env::var_os(FIXTURE_ENV) else {
        return Ok(SettingsTransport::Live(live));
    };
    let body = std::fs::read_to_string(&path)
        .map_err(|error| format!("display fixture {}: {error}", path.to_string_lossy()))?;
    let displays = FixtureDisplays::parse(&body)?;
    eprintln!("flamewm-settings: display fixture active ({FIXTURE_ENV})");
    Ok(SettingsTransport::Fixture { live, displays })
}

/// Fake display state and the recorded topology requests, in arrival order.
pub struct FixtureDisplays {
    snapshot: DisplaySnapshot,
    log: Option<PathBuf>,
    records: Vec<Value>,
    next_transaction: u64,
}

impl FixtureDisplays {
    /// Parses the fixture JSON. Each output gets one mode equal to its geometry.
    pub fn parse(body: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(body)
            .map_err(|error| format!("display fixture is not JSON: {error}"))?;
        let entries = root
            .get("outputs")
            .and_then(Value::as_array)
            .ok_or("display fixture needs an outputs array")?;
        let outputs = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| fixture_output(index, entry))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            snapshot: DisplaySnapshot {
                generation: 1,
                outputs,
                pending: None,
            },
            log: root.get("log").and_then(Value::as_str).map(PathBuf::from),
            records: Vec::new(),
            next_transaction: 1,
        })
    }

    /// Answers a display request from the fixture, or `None` when the live bus must answer it.
    pub fn answer(&mut self, request: &ControlRequest) -> Option<ControlResponse> {
        match request {
            ControlRequest::GetDisplays => Some(ControlResponse::Displays(self.snapshot.clone())),
            ControlRequest::SetDisplayArrangement {
                placements,
                expected_generation,
                ..
            } => {
                self.record(json!({
                    "op": "set_display_arrangement",
                    "expected_generation": expected_generation,
                    "placements": placements.iter().map(placement_json).collect::<Vec<_>>(),
                }));
                Some(ControlResponse::Transaction(self.begin_transaction()))
            }
            ControlRequest::KeepDisplayMode { transaction, .. } => {
                self.record(json!({"op": "keep_display_mode", "transaction": transaction.0}));
                Some(ControlResponse::Unit)
            }
            ControlRequest::RevertDisplayMode { transaction } => {
                self.record(json!({"op": "revert_display_mode", "transaction": transaction.0}));
                Some(ControlResponse::Unit)
            }
            _ => None,
        }
    }

    /// Recorded requests, oldest first.
    #[cfg(test)]
    #[must_use]
    pub fn records(&self) -> &[Value] {
        &self.records
    }

    fn begin_transaction(&mut self) -> TransactionId {
        let id = self.next_transaction;
        self.next_transaction += 1;
        TransactionId(id)
    }

    /// Appends one request and rewrites the whole log, so a killed proof still leaves valid JSON.
    fn record(&mut self, record: Value) {
        self.records.push(record);
        let Some(path) = &self.log else {
            return;
        };
        match serde_json::to_string_pretty(&self.records) {
            Ok(text) => {
                if let Err(error) = std::fs::write(path, text) {
                    eprintln!(
                        "flamewm-settings: display fixture log {}: {error}",
                        path.display()
                    );
                }
            }
            Err(error) => eprintln!("flamewm-settings: display fixture log encode: {error}"),
        }
    }
}

fn placement_json(placement: &OutputPlacement) -> Value {
    json!({"output": placement.output.as_str(), "x": placement.x, "y": placement.y})
}

fn fixture_output(index: usize, entry: &Value) -> Result<OutputSnapshot, String> {
    let connector = entry
        .get("connector")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("display fixture output {index} needs a connector"))?
        .to_owned();
    let int = |key: &str| -> Result<i32, String> {
        entry
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("display fixture output {connector} needs integer {key}"))
    };
    let (width, height) = (int("width")?, int("height")?);
    let mode = ModeId(u64::try_from(index).unwrap_or(0) + 1);
    Ok(OutputSnapshot {
        id: OutputId::new(connector.clone()),
        connector: connector.clone(),
        edid_identity: format!("fixture-{connector}"),
        connected: true,
        primary: entry
            .get("primary")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        geometry: Rect::new(int("x")?, int("y")?, width, height),
        current_mode: mode,
        modes: vec![DisplayMode {
            id: mode,
            resolution: Size::new(width, height),
            refresh_millihz: 60_000,
            preferred: true,
        }],
        shell_scale_percent: 100,
        rotation: Rotation::Normal,
    })
}

#[cfg(test)]
mod tests {
    use flamewm_api::OutputId;
    use flamewm_api::display::OutputPlacement;
    use flamewm_control_core::{ControlRequest, ControlResponse};

    use super::FixtureDisplays;

    const BODY: &str = r#"{"outputs":[
        {"connector":"DP-1","x":0,"y":0,"width":1920,"height":1080,"primary":true},
        {"connector":"HDMI-1","x":1920,"y":0,"width":1280,"height":720}]}"#;

    #[test]
    fn fixture_outputs_carry_geometry_and_primary_flag() {
        let mut fixture = FixtureDisplays::parse(BODY).expect("fixture parses");
        let Some(ControlResponse::Displays(snapshot)) =
            fixture.answer(&ControlRequest::GetDisplays)
        else {
            panic!("GetDisplays must answer a display snapshot");
        };
        assert_eq!(snapshot.outputs.len(), 2);
        assert!(snapshot.outputs[0].primary);
        assert_eq!(snapshot.outputs[1].geometry.x, 1920);
        assert_eq!(snapshot.outputs[1].connector, "HDMI-1");
    }

    #[test]
    fn arrangement_is_recorded_and_answers_a_transaction_without_the_bus() {
        let mut fixture = FixtureDisplays::parse(BODY).expect("fixture parses");
        let response = fixture.answer(&ControlRequest::SetDisplayArrangement {
            placements: vec![OutputPlacement {
                output: OutputId::new("HDMI-1"),
                x: 1920,
                y: 100,
            }],
            expected_generation: 1,
            now_ms: 5,
        });
        assert!(matches!(response, Some(ControlResponse::Transaction(_))));
        let record = &fixture.records()[0];
        assert_eq!(record["op"], "set_display_arrangement");
        assert_eq!(record["placements"][0]["output"], "HDMI-1");
        assert_eq!(record["placements"][0]["y"], 100);
    }

    #[test]
    fn non_display_requests_fall_through_to_the_live_bus() {
        let mut fixture = FixtureDisplays::parse(BODY).expect("fixture parses");
        assert!(fixture.answer(&ControlRequest::GetSettings).is_none());
        assert!(fixture.records().is_empty());
    }

    #[test]
    fn fixture_without_outputs_is_rejected() {
        assert!(FixtureDisplays::parse(r#"{"log":"x"}"#).is_err());
    }
}
