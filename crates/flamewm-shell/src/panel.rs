use flamewm_api::display::OutputSnapshot;
use flamewm_api::panels::PanelSnapshot;
use flamewm_shell_core::{start_surface_layout, StartSurfaceLayout};
use flamewm_ui_core::style::ShellMetrics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelView {
    pub snapshot: PanelSnapshot,
    pub start: StartSurfaceLayout,
}

/// One view per panel whose output is present in `outputs` (the work area needs the output rect).
#[must_use]
pub fn views(
    snapshot: &[PanelSnapshot],
    outputs: &[OutputSnapshot],
    metrics: ShellMetrics,
) -> Vec<PanelView> {
    snapshot
        .iter()
        .filter_map(|panel| {
            let output = outputs.iter().find(|output| output.id == panel.output)?;
            Some(PanelView {
                start: start_surface_layout(
                    panel.output.clone(),
                    output.geometry,
                    panel.geometry,
                    panel.edge,
                    metrics,
                ),
                snapshot: panel.clone(),
            })
        })
        .collect()
}
