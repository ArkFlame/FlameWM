//! `_NET_WM_ICON_GEOMETRY` planning: which client windows need the root-space
//! rect of their task slot, and when that value must be written or cleared.
//! Pure logic; the X11 write goes through [`IconGeometrySink`].

use std::collections::BTreeMap;

use flamewm_api::{OutputId, WindowRef};

/// Root-space `[x, y, width, height]` as the 4 CARDINAL values of the property.
pub type IconRect = [u32; 4];

/// Converts a root-space device rect to the CARDINAL form. Negative origins
/// clamp to 0 (the property cannot carry them).
#[must_use]
pub fn cardinal_rect(x: f32, y: f32, width: f32, height: f32) -> IconRect {
    [x, y, width, height].map(|value| value.round().clamp(0.0, f32::from(u16::MAX)) as u32)
}

/// Pairs each task slot's exact client reference with its root-space slot rect.
/// `slots[i]` is slot `i`'s window (None for an empty or pinned-only slot); `slot_rect(i)`
/// is that slot's root-space `(x, y, width, height)`, None when no panel has it.
#[must_use]
pub fn desired_slot_geometry(
    slots: &[Option<WindowRef>],
    slot_rect: impl Fn(usize) -> Option<(f32, f32, f32, f32)>,
) -> Vec<(WindowRef, IconRect)> {
    slots
        .iter()
        .enumerate()
        .filter_map(|(index, xid)| {
            let (x, y, width, height) = slot_rect(index)?;
            Some(((*xid)?, cardinal_rect(x, y, width, height)))
        })
        .collect()
}

/// Pairs task slots with rects from their own output's retained panel.
#[must_use]
pub fn desired_output_slot_geometry(
    outputs: &[(OutputId, Vec<Option<WindowRef>>)],
    slot_rect: impl Fn(&OutputId, usize) -> Option<(f32, f32, f32, f32)>,
) -> Vec<(WindowRef, IconRect)> {
    outputs
        .iter()
        .flat_map(|(output, slots)| {
            slots.iter().enumerate().filter_map(|(index, window)| {
                let (x, y, width, height) = slot_rect(output, index)?;
                Some(((*window)?, cardinal_rect(x, y, width, height)))
            })
        })
        .collect()
}

/// Write side of the property. Implemented by the X11 facade in production
/// and by a recorder in tests.
pub trait IconGeometrySink {
    fn set(&mut self, window: u64, rect: IconRect);
    fn clear(&mut self, window: u64);
}

/// Remembers the last value written per client window so unchanged layouts
/// produce no X writes.
#[derive(Debug, Default)]
pub struct IconGeometryTracker {
    written: BTreeMap<WindowRef, IconRect>,
}

impl IconGeometryTracker {
    /// Brings the property in line with `desired` (window -> slot rect).
    /// Writes only changed or new entries; clears windows that left `desired`.
    pub fn sync(&mut self, desired: &[(WindowRef, IconRect)], sink: &mut impl IconGeometrySink) {
        // A reused XID is the same X property but a different managed client:
        // discard the old generation without clearing the replacement value.
        self.written.retain(|window, _| {
            !desired.iter().any(|(current, _)| current.id == window.id && current != window)
        });
        let gone: Vec<WindowRef> = self
            .written
            .keys()
            .copied()
            .filter(|window| !desired.iter().any(|(id, _)| id.id == window.id))
            .collect();
        for window in gone {
            sink.clear(window.id);
            self.written.remove(&window);
        }
        for &(window, rect) in desired {
            if self.written.get(&window) != Some(&rect) {
                sink.set(window.id, rect);
                self.written.insert(window, rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        ops: Vec<(&'static str, u64, IconRect)>,
    }

    impl IconGeometrySink for Recorder {
        fn set(&mut self, window: u64, rect: IconRect) {
            self.ops.push(("set", window, rect));
        }
        fn clear(&mut self, window: u64) {
            self.ops.push(("clear", window, [0; 4]));
        }
    }

    #[test]
    fn panel_offset_is_kept_on_all_four_edges() {
        // Shell reads root-space rects (origin already includes the panel),
        // so the value must pass through unchanged for every edge.
        assert_eq!(cardinal_rect(0.0, 1000.0, 80.0, 40.0), [0, 1000, 80, 40]);
        assert_eq!(cardinal_rect(1920.0, 0.0, 80.0, 40.0), [1920, 0, 80, 40]);
        assert_eq!(cardinal_rect(10.0, 20.0, 30.0, 40.0), [10, 20, 30, 40]);
        assert_eq!(cardinal_rect(-5.0, 2.4, 9.6, 0.0), [0, 2, 10, 0]);
    }

    #[test]
    fn slot_geometry_skips_empty_slots_and_missing_rects() {
        let slots = [Some(WindowRef::new(7, 1)), None, Some(WindowRef::new(9, 1)), Some(WindowRef::new(11, 1))];
        let rects = [
            Some((10.0, 1000.0, 80.0, 40.0)),
            Some((90.0, 1000.0, 80.0, 40.0)),
            None,
            Some((170.4, 1000.0, 80.0, 40.0)),
        ];
        let desired = desired_slot_geometry(&slots, |index| rects[index]);
        assert_eq!(
            desired,
            vec![(WindowRef::new(7, 1), [10, 1000, 80, 40]), (WindowRef::new(11, 1), [170, 1000, 80, 40])]
        );
    }

    #[test]
    fn output_slots_use_their_own_panel_rects() {
        let outputs = vec![
            (flamewm_api::OutputId::new("left"), vec![Some(WindowRef::new(7, 1))]),
            (flamewm_api::OutputId::new("right"), vec![Some(WindowRef::new(9, 1))]),
        ];
        let desired = desired_output_slot_geometry(&outputs, |output, _| {
            Some(if output.as_str() == "left" {
                (20.0, 1000.0, 32.0, 40.0)
            } else {
                (1940.0, 700.0, 32.0, 40.0)
            })
        });
        assert_eq!(desired, vec![(WindowRef::new(7, 1), [20, 1000, 32, 40]), (WindowRef::new(9, 1), [1940, 700, 32, 40])]);
    }

    #[test]
    fn unchanged_layout_writes_nothing() {
        let mut tracker = IconGeometryTracker::default();
        let mut sink = Recorder::default();
        let desired = [(WindowRef::new(7, 1), [1, 2, 3, 4])];
        tracker.sync(&desired, &mut sink);
        tracker.sync(&desired, &mut sink);
        assert_eq!(sink.ops, vec![("set", 7, [1, 2, 3, 4])]);
    }

    #[test]
    fn moved_slot_rewrites_only_that_window() {
        let mut tracker = IconGeometryTracker::default();
        let mut sink = Recorder::default();
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4]), (WindowRef::new(8, 1), [5, 6, 7, 8])], &mut sink);
        sink.ops.clear();
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4]), (WindowRef::new(8, 1), [9, 6, 7, 8])], &mut sink);
        assert_eq!(sink.ops, vec![("set", 8, [9, 6, 7, 8])]);
    }

    #[test]
    fn removed_task_clears_property_once() {
        let mut tracker = IconGeometryTracker::default();
        let mut sink = Recorder::default();
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4]), (WindowRef::new(8, 1), [5, 6, 7, 8])], &mut sink);
        sink.ops.clear();
        tracker.sync(&[(WindowRef::new(8, 1), [5, 6, 7, 8])], &mut sink);
        tracker.sync(&[(WindowRef::new(8, 1), [5, 6, 7, 8])], &mut sink);
        assert_eq!(sink.ops, vec![("clear", 7, [0; 4])]);
    }

    #[test]
    fn reappearing_window_is_written_again_after_clear() {
        let mut tracker = IconGeometryTracker::default();
        let mut sink = Recorder::default();
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4])], &mut sink);
        tracker.sync(&[], &mut sink);
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4])], &mut sink);
        assert_eq!(
            sink.ops,
            vec![
                ("set", 7, [1, 2, 3, 4]),
                ("clear", 7, [0; 4]),
                ("set", 7, [1, 2, 3, 4]),
            ]
        );
    }

    #[test]
    fn recycled_xid_rewrites_even_when_slot_rect_is_unchanged() {
        let mut tracker = IconGeometryTracker::default();
        let mut sink = Recorder::default();
        tracker.sync(&[(WindowRef::new(7, 1), [1, 2, 3, 4])], &mut sink);
        sink.ops.clear();
        tracker.sync(&[(WindowRef::new(7, 2), [1, 2, 3, 4])], &mut sink);
        assert_eq!(sink.ops, vec![("set", 7, [1, 2, 3, 4])]);
    }
}
