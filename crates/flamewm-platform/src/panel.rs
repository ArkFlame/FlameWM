use std::collections::BTreeMap;

use flamewm_api::{OutputId, PanelEdge, Rect};

pub const DEFAULT_PANEL_LOGICAL_SIZE: u16 = 44;
pub const MIN_PANEL_LOGICAL_SIZE: u16 = 34;
pub const MAX_PANEL_LOGICAL_SIZE: u16 = 72;
pub const SUPPORTED_SCALES: [u16; 5] = [100, 125, 150, 175, 200];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Strut {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

#[must_use]
pub const fn supported_scale(scale_percent: u16) -> bool {
    matches!(scale_percent, 100 | 125 | 150 | 175 | 200)
}

#[must_use]
pub fn logical_to_physical(logical: u16, scale_percent: u16) -> i32 {
    let scale = if supported_scale(scale_percent) {
        scale_percent
    } else {
        100
    };
    ((u32::from(logical) * u32::from(scale) + 50) / 100) as i32
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPanel {
    pub output: OutputId,
    pub output_rect: Rect,
    pub edge: PanelEdge,
    pub scale_percent: u16,
    pub logical_size: u16,
}

impl OutputPanel {
    #[must_use]
    pub fn new(output: OutputId, output_rect: Rect, edge: PanelEdge, scale_percent: u16) -> Self {
        Self {
            output,
            output_rect,
            edge,
            scale_percent: if supported_scale(scale_percent) {
                scale_percent
            } else {
                100
            },
            logical_size: DEFAULT_PANEL_LOGICAL_SIZE,
        }
    }

    pub fn set_logical_size(&mut self, size: u16) -> bool {
        if !(MIN_PANEL_LOGICAL_SIZE..=MAX_PANEL_LOGICAL_SIZE).contains(&size) {
            return false;
        }
        self.logical_size = size;
        true
    }

    #[must_use]
    pub fn thickness(&self) -> i32 {
        logical_to_physical(self.logical_size, self.scale_percent)
    }

    #[must_use]
    pub fn panel_rect(&self) -> Rect {
        let thickness = self.thickness();
        match self.edge {
            PanelEdge::Bottom => Rect::new(
                self.output_rect.x,
                self.output_rect.bottom() - thickness,
                self.output_rect.width,
                thickness,
            ),
            PanelEdge::Top => Rect::new(
                self.output_rect.x,
                self.output_rect.y,
                self.output_rect.width,
                thickness,
            ),
            PanelEdge::Left => Rect::new(
                self.output_rect.x,
                self.output_rect.y,
                thickness,
                self.output_rect.height,
            ),
            PanelEdge::Right => Rect::new(
                self.output_rect.right() - thickness,
                self.output_rect.y,
                thickness,
                self.output_rect.height,
            ),
        }
    }

    #[must_use]
    pub fn strut(&self) -> Strut {
        let thickness = self.thickness();
        match self.edge {
            PanelEdge::Bottom => Strut {
                bottom: thickness,
                ..Strut::default()
            },
            PanelEdge::Top => Strut {
                top: thickness,
                ..Strut::default()
            },
            PanelEdge::Left => Strut {
                left: thickness,
                ..Strut::default()
            },
            PanelEdge::Right => Strut {
                right: thickness,
                ..Strut::default()
            },
        }
    }

    #[must_use]
    pub fn work_area(&self) -> Rect {
        let thickness = self.thickness();
        match self.edge {
            PanelEdge::Bottom => Rect::new(
                self.output_rect.x,
                self.output_rect.y,
                self.output_rect.width,
                self.output_rect.height - thickness,
            ),
            PanelEdge::Top => Rect::new(
                self.output_rect.x,
                self.output_rect.y + thickness,
                self.output_rect.width,
                self.output_rect.height - thickness,
            ),
            PanelEdge::Left => Rect::new(
                self.output_rect.x + thickness,
                self.output_rect.y,
                self.output_rect.width - thickness,
                self.output_rect.height,
            ),
            PanelEdge::Right => Rect::new(
                self.output_rect.x,
                self.output_rect.y,
                self.output_rect.width - thickness,
                self.output_rect.height,
            ),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PanelManager {
    panels: BTreeMap<OutputId, OutputPanel>,
    focused_frame_output: Option<OutputId>,
    pointer_output: Option<OutputId>,
    primary_output: Option<OutputId>,
    generation: u64,
}

impl PanelManager {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn upsert_output(&mut self, panel: OutputPanel) {
        let changed = self.panels.get(&panel.output) != Some(&panel);
        if changed {
            self.panels.insert(panel.output.clone(), panel);
            self.generation += 1;
        }
    }

    pub fn remove_output(&mut self, output: &OutputId) -> bool {
        if self.panels.remove(output).is_none() {
            return false;
        }
        if self.focused_frame_output.as_ref() == Some(output) {
            self.focused_frame_output = None;
        }
        if self.pointer_output.as_ref() == Some(output) {
            self.pointer_output = None;
        }
        if self.primary_output.as_ref() == Some(output) {
            self.primary_output = None;
        }
        self.generation += 1;
        true
    }

    pub fn set_focused_frame_output(&mut self, output: Option<OutputId>) {
        self.focused_frame_output = output;
    }

    pub fn set_pointer_output(&mut self, output: Option<OutputId>) {
        self.pointer_output = output;
    }

    pub fn set_primary_output(&mut self, output: Option<OutputId>) {
        self.primary_output = output;
    }

    #[must_use]
    pub fn active_output(&self) -> Option<OutputId> {
        // Current native Shell contract: focused window output, then primary, then first.
        // Pointer location no longer redirects keyboard Start toggling.
        for candidate in [
            self.focused_frame_output.as_ref(),
            self.primary_output.as_ref(),
        ] {
            if let Some(output) = candidate {
                if self.panels.contains_key(output) {
                    return Some(output.clone());
                }
            }
        }
        self.panels.keys().next().cloned()
    }

    #[must_use]
    pub fn panel(&self, output: &OutputId) -> Option<&OutputPanel> {
        self.panels.get(output)
    }

    pub fn panel_mut(&mut self, output: &OutputId) -> Option<&mut OutputPanel> {
        self.panels.get_mut(output)
    }

    #[must_use]
    pub fn outputs(&self) -> Vec<OutputId> {
        self.panels.keys().cloned().collect()
    }

    #[must_use]
    pub fn panels(&self) -> impl Iterator<Item = &OutputPanel> {
        self.panels.values()
    }

    #[must_use]
    pub fn primary_output(&self) -> Option<&OutputId> {
        self.primary_output.as_ref()
    }

    #[must_use]
    pub fn work_areas(&self) -> Vec<(OutputId, Rect)> {
        self.panels
            .iter()
            .map(|(output, panel)| (output.clone(), panel.work_area()))
            .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartController {
    open_output: Option<OutputId>,
}

impl StartController {
    /// Toggle Start on the resolved output. At most one Start surface is open globally.
    pub fn toggle(&mut self, active_output: Option<OutputId>) -> bool {
        let Some(target) = active_output else {
            self.open_output = None;
            return false;
        };
        if self.open_output.as_ref() == Some(&target) {
            self.open_output = None;
            return false;
        }
        self.open_output = Some(target);
        true
    }

    pub fn close(&mut self) {
        self.open_output = None;
    }

    pub fn output_removed(&mut self, output: &OutputId) {
        if self.open_output.as_ref() == Some(output) {
            self.close();
        }
    }

    #[must_use]
    pub fn open_output(&self) -> Option<&OutputId> {
        self.open_output.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> OutputId {
        OutputId::new(value)
    }

    #[test]
    fn panel_geometry_supports_all_four_edges_and_negative_origins() {
        let output = Rect::new(-1920, 0, 1920, 1080);
        let right = OutputPanel::new(id("HDMI-1"), output, PanelEdge::Right, 100);
        assert_eq!(right.panel_rect(), Rect::new(-44, 0, 44, 1080));
        assert_eq!(right.strut().right, 44);
        assert_eq!(right.work_area(), Rect::new(-1920, 0, 1876, 1080));
    }

    #[test]
    fn default_logical_panel_size_scales_to_physical_geometry() {
        let output = Rect::new(0, 0, 1920, 1080);
        for (scale, expected) in [(100, 44), (125, 55), (150, 66), (175, 77), (200, 88)] {
            let panel = OutputPanel::new(id("eDP-1"), output, PanelEdge::Bottom, scale);
            assert_eq!(panel.logical_size, 44);
            assert_eq!(panel.thickness(), expected);
            assert_eq!(
                panel.panel_rect(),
                Rect::new(0, 1080 - expected, 1920, expected)
            );
        }
    }

    #[test]
    fn unsupported_panel_scale_falls_back_to_one_hundred_percent() {
        assert!(!supported_scale(110));
        let panel = OutputPanel::new(
            id("eDP-1"),
            Rect::new(0, 0, 1920, 1080),
            PanelEdge::Bottom,
            110,
        );
        assert_eq!(panel.scale_percent, 100);
        assert_eq!(panel.thickness(), 44);
    }

    #[test]
    fn active_output_resolution_is_focus_then_primary_then_first() {
        let mut manager = PanelManager::default();
        manager.upsert_output(OutputPanel::new(
            id("eDP-1"),
            Rect::new(0, 0, 1920, 1080),
            PanelEdge::Bottom,
            100,
        ));
        manager.upsert_output(OutputPanel::new(
            id("HDMI-1"),
            Rect::new(1920, 0, 2560, 1440),
            PanelEdge::Bottom,
            100,
        ));
        manager.set_primary_output(Some(id("eDP-1")));
        manager.set_pointer_output(Some(id("HDMI-1")));
        assert_eq!(manager.active_output(), Some(id("eDP-1")));
        manager.set_focused_frame_output(Some(id("HDMI-1")));
        assert_eq!(manager.active_output(), Some(id("HDMI-1")));
    }

    #[test]
    fn start_is_single_open_and_toggle_is_output_local() {
        let mut start = StartController::default();
        assert!(start.toggle(Some(id("eDP-1"))));
        assert_eq!(start.open_output(), Some(&id("eDP-1")));
        assert!(start.toggle(Some(id("HDMI-1"))));
        assert_eq!(start.open_output(), Some(&id("HDMI-1")));
        assert!(!start.toggle(Some(id("HDMI-1"))));
        assert_eq!(start.open_output(), None);
    }
}
