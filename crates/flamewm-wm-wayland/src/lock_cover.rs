//! Per-output coverage of a held session lock. Pure: outputs are keyed by connector name, so every
//! rule is unit-tested without smithay, a GPU or hardware.
//!
//! A lock covers the session only when every output in the layout has a live lock surface. An
//! output that appears while locked starts uncovered, and a dead locker uncovers its outputs.
//! Desktop input gating is not here: `session_lock::windows_reachable` closes it for the whole
//! lock, covered or not, so an uncovered output shows nothing, never the desktop.
#![allow(dead_code)]

use std::collections::BTreeSet;

/// The outputs of the layout and the subset a live lock surface covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockCover {
    outputs: BTreeSet<String>,
    covered: BTreeSet<String>,
}

impl LockCover {
    /// A fresh cover for `outputs`, with nothing covered yet.
    pub fn new<I, S>(outputs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            outputs: outputs.into_iter().map(Into::into).collect(),
            covered: BTreeSet::new(),
        }
    }

    /// Records a live lock surface on `output`. Returns `false` for an output not in the layout,
    /// which cannot be covered.
    pub fn cover(&mut self, output: &str) -> bool {
        if !self.outputs.contains(output) {
            return false;
        }
        self.covered.insert(output.to_owned());
        true
    }

    /// Forgets the lock surface on `output`, for example because its client died.
    pub fn uncover(&mut self, output: &str) {
        self.covered.remove(output);
    }

    /// An output was connected. It is uncovered until its own lock surface arrives.
    pub fn output_added(&mut self, output: &str) {
        self.outputs.insert(output.to_owned());
    }

    /// An output was disconnected. It no longer needs covering and any cover it had is dropped.
    pub fn output_removed(&mut self, output: &str) {
        self.outputs.remove(output);
        self.covered.remove(output);
    }

    /// Whether the lock covers every output. An empty layout is never complete: nothing is shown.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.outputs.is_empty() && self.outputs.iter().all(|o| self.covered.contains(o))
    }

    /// Outputs in the layout without a live lock surface, in name order.
    #[must_use]
    pub fn uncovered(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|o| !self.covered.contains(*o))
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_cover_is_incomplete_until_every_output_has_a_surface() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        assert!(!cover.is_complete());
        assert_eq!(cover.uncovered(), ["DP-1", "HDMI-A-1"]);
        assert!(cover.cover("DP-1"));
        assert!(!cover.is_complete());
        assert_eq!(cover.uncovered(), ["HDMI-A-1"]);
        assert!(cover.cover("HDMI-A-1"));
        assert!(cover.is_complete());
        assert!(cover.uncovered().is_empty());
    }

    #[test]
    fn a_surface_for_an_unknown_output_covers_nothing() {
        let mut cover = LockCover::new(["DP-1"]);
        assert!(!cover.cover("eDP-1"));
        assert!(!cover.is_complete());
    }

    #[test]
    fn an_empty_layout_is_never_complete() {
        let cover = LockCover::new(Vec::<String>::new());
        assert!(!cover.is_complete());
    }

    #[test]
    fn a_dead_locker_uncovers_its_output_again() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        cover.cover("DP-1");
        cover.cover("HDMI-A-1");
        cover.uncover("HDMI-A-1");
        assert!(!cover.is_complete());
        assert_eq!(cover.uncovered(), ["HDMI-A-1"]);
    }

    #[test]
    fn an_output_added_while_locked_starts_uncovered() {
        let mut cover = LockCover::new(["DP-1"]);
        cover.cover("DP-1");
        assert!(cover.is_complete());
        cover.output_added("HDMI-A-1");
        assert!(!cover.is_complete());
        assert_eq!(cover.uncovered(), ["HDMI-A-1"]);
        cover.cover("HDMI-A-1");
        assert!(cover.is_complete());
    }

    #[test]
    fn removing_an_uncovered_output_can_complete_the_lock() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        cover.cover("DP-1");
        cover.output_removed("HDMI-A-1");
        assert!(cover.is_complete());
    }

    #[test]
    fn removing_a_covered_output_drops_its_cover() {
        let mut cover = LockCover::new(["DP-1"]);
        cover.cover("DP-1");
        cover.output_removed("DP-1");
        assert!(!cover.is_complete());
        cover.output_added("DP-1");
        assert_eq!(cover.uncovered(), ["DP-1"]);
    }
}
