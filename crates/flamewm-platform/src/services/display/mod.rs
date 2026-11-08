use std::collections::BTreeMap;

use flamewm_api::display::{
    DisplaySnapshot, OutputPlacement, OutputSnapshot, PendingModeChange, TopologyChange,
};
use flamewm_api::geometry::Rect;
use flamewm_api::ports::DisplayPort;
use flamewm_api::{ErrorCode, FlameError, FlameResult, ModeId, OutputId, TransactionId};

use crate::display::DisplayCoordinator;
use crate::scale::is_supported_scale;

#[derive(Debug, Default)]
pub struct DisplayService {
    coordinator: DisplayCoordinator,
    cached: Option<DisplaySnapshot>,
    engine_generation: u64,
    /// Last shell scale set per output ID, kept across unplug so a re-plugged output keeps it.
    scale_memory: BTreeMap<OutputId, u16>,
}

use refresh::require_generation;
use scale::{restore_shell_scales, scale_map};
use transaction::to_api_pending;

mod mode;
mod refresh;
mod scale;
mod topology;
mod transaction;

#[cfg(test)]
mod tests_five;
#[cfg(test)]
mod tests_mode;
#[cfg(test)]
mod tests_output;
#[cfg(test)]
mod tests_topology;
