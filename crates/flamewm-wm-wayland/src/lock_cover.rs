//! Per-output coverage of a held session lock. Pure: outputs are keyed by connector name, so every
//! rule is unit-tested without smithay, a GPU or hardware.
//!
//! A lock covers the session only when every output in the layout has an opaque cover: either a
//! live lock surface from the client, or the compositor's full-output placeholder when the client
//! did not supply one for that output. An output that appears while locked starts with a
//! placeholder, and a dead locker leaves its placeholder in place. Desktop input gating is not
//! here: `session_lock::windows_reachable` closes it for the whole lock, covered or not, so an
//! uncovered output shows a cover, never the desktop.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

/// What keeps one output opaque while the session is locked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverKind {
    /// A live `ext-session-lock-v1` surface supplied by the lock client.
    Surface,
    /// A full-output opaque element painted by the compositor when the client supplied no surface.
    Placeholder,
}

/// The outputs of the layout and what covers each of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockCover {
    outputs: BTreeSet<String>,
    covers: BTreeMap<String, CoverKind>,
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
            covers: BTreeMap::new(),
        }
    }

    /// Records a live lock surface on `output`. Returns `false` for an output not in the layout,
    /// which cannot be covered. A placeholder on the output is replaced by the real surface.
    pub fn cover(&mut self, output: &str) -> bool {
        if !self.outputs.contains(output) {
            return false;
        }
        self.covers.insert(output.to_owned(), CoverKind::Surface);
        true
    }

    /// Records a compositor-drawn opaque placeholder on `output`. It never downgrades a live lock
    /// surface. Returns `false` for an output not in the layout.
    pub fn cover_placeholder(&mut self, output: &str) -> bool {
        if !self.outputs.contains(output) {
            return false;
        }
        self.covers
            .entry(output.to_owned())
            .or_insert(CoverKind::Placeholder);
        true
    }

    /// Gives every output of the layout an opaque cover. Outputs without a live lock surface get a
    /// placeholder, so no output can show the desktop while the lock is held.
    pub fn cover_all_uncovered(&mut self) {
        for output in &self.outputs {
            self.covers
                .entry(output.clone())
                .or_insert(CoverKind::Placeholder);
        }
    }

    /// Forgets the cover on `output`, for example because its client died. A caller that keeps the
    /// session locked should put a placeholder back with `cover_placeholder`.
    pub fn uncover(&mut self, output: &str) {
        self.covers.remove(output);
    }

    /// An output was connected. It is covered by a placeholder until its own lock surface arrives,
    /// even when a cover from a previous connection of the same connector lingers.
    pub fn output_added(&mut self, output: &str) {
        self.outputs.insert(output.to_owned());
        self.covers
            .insert(output.to_owned(), CoverKind::Placeholder);
    }

    /// An output was disconnected. It no longer needs covering and any cover it had is dropped.
    pub fn output_removed(&mut self, output: &str) {
        self.outputs.remove(output);
        self.covers.remove(output);
    }

    /// How `output` is covered, if at all.
    #[must_use]
    pub fn cover_for(&self, output: &str) -> Option<CoverKind> {
        self.covers.get(output).copied()
    }

    /// Whether every output has an opaque cover, a client surface or a placeholder. An empty
    /// layout is never complete: nothing is shown.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.outputs.is_empty() && self.outputs.iter().all(|o| self.covers.contains_key(o))
    }

    /// Whether every output got a real lock surface from the client. Placeholders keep the session
    /// opaque but are not a finished lock handshake.
    #[must_use]
    pub fn all_surfaces(&self) -> bool {
        !self.outputs.is_empty()
            && self
                .outputs
                .iter()
                .all(|o| self.covers.get(o) == Some(&CoverKind::Surface))
    }

    /// Outputs in the layout without any opaque cover, in name order.
    #[must_use]
    pub fn uncovered(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|o| !self.covers.contains_key(*o))
            .map(String::as_str)
            .collect()
    }

    /// Outputs still waiting for a client lock surface, in name order. They are already covered by
    /// an opaque placeholder, so they never show the desktop.
    #[must_use]
    pub fn placeholder_outputs(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|o| self.covers.get(*o) == Some(&CoverKind::Placeholder))
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
        assert_eq!(cover.uncovered().len(), 0);
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
    fn an_output_added_while_locked_starts_with_a_placeholder() {
        let mut cover = LockCover::new(["DP-1"]);
        cover.cover("DP-1");
        assert!(cover.all_surfaces());
        cover.output_added("HDMI-A-1");
        // The new output is covered immediately, but by the compositor, not by the client.
        assert!(cover.is_complete());
        assert!(!cover.all_surfaces());
        assert_eq!(cover.cover_for("HDMI-A-1"), Some(CoverKind::Placeholder));
        assert_eq!(cover.placeholder_outputs(), ["HDMI-A-1"]);
        cover.cover("HDMI-A-1");
        assert!(cover.all_surfaces());
        assert_eq!(cover.placeholder_outputs().len(), 0);
    }

    #[test]
    fn a_reconnected_output_loses_its_lingering_surface() {
        // Resume drops the output and re-adds the same connector while locked: its old surface
        // must not make the new output look covered by the client.
        let mut cover = LockCover::new(["DP-1"]);
        cover.cover("DP-1");
        cover.output_added("DP-1");
        assert!(cover.is_complete());
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Placeholder));
        assert!(!cover.all_surfaces());
    }

    #[test]
    fn placeholders_cover_every_output_without_a_surface() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1", "eDP-1"]);
        cover.cover("DP-1");
        cover.cover_all_uncovered();
        assert!(cover.is_complete());
        assert!(!cover.all_surfaces());
        assert_eq!(cover.uncovered().len(), 0);
        assert_eq!(cover.placeholder_outputs(), ["HDMI-A-1", "eDP-1"]);
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Surface));
    }

    #[test]
    fn a_real_surface_replaces_its_outputs_placeholder() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        cover.cover_all_uncovered();
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Placeholder));
        assert!(cover.cover("DP-1"));
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Surface));
        assert_eq!(cover.placeholder_outputs(), ["HDMI-A-1"]);
    }

    #[test]
    fn covering_all_uncovered_does_not_downgrade_live_surfaces() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        cover.cover("DP-1");
        cover.cover_all_uncovered();
        cover.cover_all_uncovered();
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Surface));
        assert_eq!(cover.placeholder_outputs(), ["HDMI-A-1"]);
    }

    #[test]
    fn a_placeholder_for_an_unknown_output_covers_nothing() {
        let mut cover = LockCover::new(["DP-1"]);
        assert!(!cover.cover_placeholder("eDP-1"));
        assert!(!cover.is_complete());
    }

    #[test]
    fn a_placeholder_never_replaces_a_live_surface() {
        let mut cover = LockCover::new(["DP-1"]);
        cover.cover("DP-1");
        assert!(cover.cover_placeholder("DP-1"));
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Surface));
    }

    #[test]
    fn a_dead_lockers_output_keeps_its_placeholder() {
        let mut cover = LockCover::new(["DP-1", "HDMI-A-1"]);
        cover.cover_all_uncovered();
        cover.cover("DP-1");
        cover.cover("HDMI-A-1");
        // prune_dead_locks uncovers the dead client's output and immediately puts a placeholder back.
        cover.uncover("HDMI-A-1");
        cover.cover_placeholder("HDMI-A-1");
        assert!(cover.is_complete());
        assert!(!cover.all_surfaces());
        assert_eq!(cover.placeholder_outputs(), ["HDMI-A-1"]);
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
        assert_eq!(cover.cover_for("DP-1"), Some(CoverKind::Placeholder));
    }
}
