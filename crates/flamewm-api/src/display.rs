use crate::{ModeId, OutputId, Rect, Size, TransactionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayMode {
    pub id: ModeId,
    pub resolution: Size,
    pub refresh_millihz: i32,
    pub preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub id: OutputId,
    pub connector: String,
    pub edid_identity: String,
    pub connected: bool,
    pub primary: bool,
    pub geometry: Rect,
    pub current_mode: ModeId,
    pub modes: Vec<DisplayMode>,
    pub shell_scale_percent: u16,
    /// Current panel rotation; the WM reports it, peers that omit it read as `Normal`.
    pub rotation: Rotation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingModeChange {
    pub transaction: TransactionId,
    pub output: OutputId,
    pub mode: ModeId,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySnapshot {
    pub generation: u64,
    pub outputs: Vec<OutputSnapshot>,
    pub pending: Option<PendingModeChange>,
}

/// Panel rotation of one output, in clockwise quarter turns from the native orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    Normal,
    Left,
    Right,
    Inverted,
}

/// Desired top-left origin of one connected output in the shared desktop coordinate space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPlacement {
    pub output: OutputId,
    pub x: i32,
    pub y: i32,
}

/// Topology change that runs through the same keep/revert transaction as a mode change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyChange {
    SetPrimary(OutputId),
    SetOrientation {
        output: OutputId,
        rotation: Rotation,
    },
    SetArrangement(Vec<OutputPlacement>),
}

impl TopologyChange {
    /// Output that anchors the pending transaction: the target output, or the first placement.
    #[must_use]
    pub fn anchor(&self) -> Option<&OutputId> {
        match self {
            Self::SetPrimary(output) | Self::SetOrientation { output, .. } => Some(output),
            Self::SetArrangement(placements) => {
                placements.first().map(|placement| &placement.output)
            }
        }
    }
}
