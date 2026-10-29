//! Grid placement for desktop widgets: cell metrics, spans, snapping, collisions,
//! migration and reflow. Pure math only; the widget store and UI call these functions.
//!
//! A placement is a cell index counted from the left margin (`right == false`) or from
//! the right margin (`right == true`). Collisions are decided on pixel rectangles so
//! left and right placements share one occupancy check.

use flamewm_api::{Point, Rect};

use crate::widgets::WidgetKind;

/// Target cell edge in logical pixels at 100% scale.
pub const TARGET_CELL: i32 = 104;
/// Smallest cell edge used when the work area is short, in logical pixels.
pub const MIN_CELL: i32 = 80;
/// Uniform gutter between cells, in logical pixels.
pub const GUTTER: i32 = 8;
/// Distance from the work-area edge to the first cell, in logical pixels.
pub const MARGIN: i32 = 16;
/// Share of the work-area width used by the widget column block, in percent.
pub const WIDGET_WIDTH_PERCENT: i32 = 30;
pub const MIN_COLUMNS: i32 = 3;
pub const MAX_COLUMNS: i32 = 6;
/// The cell edge shrinks until at least this many rows fit.
pub const MIN_ROWS: i32 = 5;

/// Widget size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub w: i32,
    pub h: i32,
}

impl Span {
    pub const SINGLE: Self = Self { w: 1, h: 1 };
    pub const WIDE: Self = Self { w: 2, h: 1 };
    pub const TALL: Self = Self { w: 1, h: 2 };
    pub const LARGE: Self = Self { w: 2, h: 2 };
    pub const ALL: [Self; 4] = [Self::SINGLE, Self::WIDE, Self::TALL, Self::LARGE];

    #[must_use]
    pub fn label(self) -> String {
        format!("{}x{}", self.w, self.h)
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|span| span.label() == label)
    }
}

/// Default size of each widget kind in the grid.
#[must_use]
pub const fn default_span(kind: WidgetKind) -> Span {
    match kind {
        WidgetKind::Clock
        | WidgetKind::Cpu
        | WidgetKind::Memory
        | WidgetKind::History
        | WidgetKind::Network
        | WidgetKind::Ring => Span::SINGLE,
        WidgetKind::Media | WidgetKind::Custom => Span::WIDE,
        WidgetKind::Weather | WidgetKind::Calendar => Span::LARGE,
    }
}

/// A widget's cell position. `col` counts from the side named by `right`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub col: i32,
    pub row: i32,
    pub span: Span,
    pub right: bool,
}

impl Placement {
    #[must_use]
    pub const fn left(col: i32, row: i32, span: Span) -> Self {
        Self { col, row, span, right: false }
    }

    #[must_use]
    pub const fn fits(self, metrics: GridMetrics) -> bool {
        self.col >= 0
            && self.row >= 0
            && self.span.w >= 1
            && self.span.h >= 1
            && self.col + self.span.w <= metrics.cols
            && self.row + self.span.h <= metrics.rows
    }
}

/// Cell geometry derived from one work area and one scale factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridMetrics {
    pub work: Rect,
    /// Cell edge in device pixels.
    pub cell: i32,
    /// Gutter in device pixels; `cell + gutter` is the pitch.
    pub gutter: i32,
    /// Margin from the work-area edge in device pixels.
    pub margin: i32,
    pub cols: i32,
    pub rows: i32,
}

fn scale_px(logical: i32, scale_percent: i32) -> i32 {
    (logical * scale_percent + 50) / 100
}

fn rows_for(height: i32, margin: i32, cell: i32, gutter: i32) -> i32 {
    ((height - 2 * margin + gutter) / (cell + gutter)).max(0)
}

impl GridMetrics {
    /// Builds the grid for `work` (device pixels) at `scale_percent` (100 = 1x).
    ///
    /// The cell starts at `TARGET_CELL` and shrinks only while fewer than `MIN_ROWS`
    /// rows fit. Columns are the number of pitches that fit in `WIDGET_WIDTH_PERCENT`
    /// of the work width, clamped to `MIN_COLUMNS..=MAX_COLUMNS`.
    #[must_use]
    pub fn for_work_area(work: Rect, scale_percent: i32) -> Self {
        let scale = scale_percent.clamp(50, 400);
        let gutter = scale_px(GUTTER, scale);
        let margin = scale_px(MARGIN, scale);
        let min_cell = scale_px(MIN_CELL, scale);
        let mut cell = scale_px(TARGET_CELL, scale);
        while cell > min_cell && rows_for(work.height, margin, cell, gutter) < MIN_ROWS {
            cell -= 1;
        }
        let rows = rows_for(work.height, margin, cell, gutter);
        let share = work.width * WIDGET_WIDTH_PERCENT / 100;
        let cols = ((share + gutter) / (cell + gutter)).clamp(MIN_COLUMNS, MAX_COLUMNS);
        Self { work, cell, gutter, margin, cols, rows }
    }

    #[must_use]
    pub const fn pitch(self) -> i32 {
        self.cell + self.gutter
    }

    /// Pixel size of a span: `n` cells plus `n - 1` gutters per axis.
    #[must_use]
    pub const fn span_px(self, span: Span) -> (i32, i32) {
        (
            span.w * self.cell + (span.w - 1) * self.gutter,
            span.h * self.cell + (span.h - 1) * self.gutter,
        )
    }

    /// Pixel rectangle of a placement inside the work area.
    #[must_use]
    pub fn rect_of(self, placement: Placement) -> Rect {
        let (width, height) = self.span_px(placement.span);
        let offset = placement.col * self.pitch();
        let x = if placement.right {
            self.work.right() - self.margin - offset - width
        } else {
            self.work.x + self.margin + offset
        };
        let y = self.work.y + self.margin + placement.row * self.pitch();
        Rect::new(x, y, width, height)
    }

    /// Span whose pixel size is closest to `width` x `height` (ties keep `Span::ALL` order).
    #[must_use]
    pub fn span_for_px(self, width: i32, height: i32) -> Span {
        Span::ALL
            .into_iter()
            .min_by_key(|span| {
                let (w, h) = self.span_px(*span);
                (w - width).abs() + (h - height).abs()
            })
            .unwrap_or(Span::SINGLE)
    }

    /// Nearest placement on `want`'s side whose top-left is closest to `point`.
    /// The result is not checked for collisions.
    #[must_use]
    pub fn snap(self, point: Point, span: Span, right: bool) -> Placement {
        let pitch = self.pitch();
        let (width, _) = self.span_px(span);
        let col_px = if right {
            self.work.right() - self.margin - (point.x + width)
        } else {
            point.x - (self.work.x + self.margin)
        };
        let row_px = point.y - (self.work.y + self.margin);
        let col = (col_px + pitch / 2).div_euclid(pitch).clamp(0, self.cols - span.w);
        let row = (row_px + pitch / 2).div_euclid(pitch).clamp(0, self.rows - span.h);
        Placement { col, row, span, right }
    }

    /// Whether two placements overlap in pixels. Gutters keep neighbours apart.
    #[must_use]
    pub fn collides(self, a: Placement, b: Placement) -> bool {
        let ra = self.rect_of(a);
        let rb = self.rect_of(b);
        ra.x < rb.right() && rb.x < ra.right() && ra.y < rb.bottom() && rb.y < ra.bottom()
    }

    fn candidates(self, span: Span, right: bool) -> impl Iterator<Item = Placement> {
        let cols = (self.cols - span.w).max(-1);
        let rows = (self.rows - span.h).max(-1);
        (0..=rows).flat_map(move |row| {
            (0..=cols).map(move |col| Placement { col, row, span, right })
        })
    }

    /// First free cell in row-major order, on the left side.
    #[must_use]
    pub fn first_free(self, span: Span, occupied: &[Placement]) -> Option<Placement> {
        self.candidates(span, false)
            .find(|candidate| occupied.iter().all(|other| !self.collides(*candidate, *other)))
    }

    /// Free cell closest (Manhattan distance in cells) to `want`, on `want`'s side.
    #[must_use]
    pub fn nearest_free(self, want: Placement, occupied: &[Placement]) -> Option<Placement> {
        self.candidates(want.span, want.right)
            .filter(|candidate| occupied.iter().all(|other| !self.collides(*candidate, *other)))
            .min_by_key(|candidate| {
                (
                    (candidate.col - want.col).abs() + (candidate.row - want.row).abs(),
                    candidate.row,
                    candidate.col,
                )
            })
    }

    /// Re-places widgets after the grid changed. Kept items retain their slot when they
    /// still fit and do not overlap an earlier kept item; the rest move to the nearest
    /// free cell, or `None` when the grid has no room left.
    #[must_use]
    pub fn reflow(self, items: &[Placement]) -> Vec<Option<Placement>> {
        let mut placed: Vec<Placement> = Vec::with_capacity(items.len());
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let kept = item.fits(self) && placed.iter().all(|other| !self.collides(*item, *other));
            let slot = if kept {
                Some(*item)
            } else {
                self.nearest_free(*item, &placed)
            };
            if let Some(slot) = slot {
                placed.push(slot);
            }
            out.push(slot);
        }
        out
    }

    /// Cell for a widget whose free pixel rect is `rect` (legacy absolute position).
    /// The side follows the rect centre, as the edge anchoring does.
    #[must_use]
    pub fn migrate(self, rect: Rect, span: Span, occupied: &[Placement]) -> Option<Placement> {
        let right = rect.x + rect.width / 2 > self.work.x + self.work.width / 2;
        let want = self.snap(rect.origin(), span, right);
        self.nearest_free(want, occupied)
    }
}

/// Default arrangement for a fresh profile, in the order the widgets are added.
const DEFAULT_SLOTS: [(WidgetKind, i32, i32); 9] = [
    (WidgetKind::Media, 0, 0),
    (WidgetKind::Custom, 2, 0),
    (WidgetKind::Clock, 0, 1),
    (WidgetKind::Weather, 2, 1),
    (WidgetKind::History, 4, 1),
    (WidgetKind::Network, 4, 2),
    (WidgetKind::Calendar, 0, 2),
    (WidgetKind::Cpu, 2, 3),
    (WidgetKind::Memory, 3, 3),
];

/// Places the default arrangement on `metrics`. Slots that no longer fit (smaller
/// grids) move to the nearest free cell, and `None` means the grid is full.
#[must_use]
pub fn default_layout(metrics: GridMetrics) -> Vec<(WidgetKind, Option<Placement>)> {
    let mut placed: Vec<Placement> = Vec::with_capacity(DEFAULT_SLOTS.len());
    let mut out = Vec::with_capacity(DEFAULT_SLOTS.len());
    for (kind, col, row) in DEFAULT_SLOTS {
        let want = Placement::left(col, row, default_span(kind));
        let slot = if want.fits(metrics) && placed.iter().all(|other| !metrics.collides(want, *other)) {
            Some(want)
        } else {
            metrics.nearest_free(want, &placed)
        };
        if let Some(slot) = slot {
            placed.push(slot);
        }
        out.push((kind, slot));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(width: i32, height: i32) -> Rect {
        Rect::new(0, 0, width, height)
    }

    #[test]
    fn full_hd_at_100_percent_gives_five_by_nine_cells() {
        // 1080 minus a 36 px taskbar strut.
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        assert_eq!((metrics.cell, metrics.gutter, metrics.margin), (104, 8, 16));
        assert_eq!((metrics.cols, metrics.rows), (5, 9));
        assert_eq!(metrics.pitch(), 112);
    }

    #[test]
    fn qhd_at_100_percent_caps_columns_at_six() {
        let metrics = GridMetrics::for_work_area(work(2560, 1404), 100);
        assert_eq!((metrics.cell, metrics.cols, metrics.rows), (104, 6, 12));
    }

    #[test]
    fn hd_laptop_keeps_three_columns_and_six_rows() {
        let metrics = GridMetrics::for_work_area(work(1366, 732), 100);
        assert_eq!((metrics.cols, metrics.rows), (3, 6));
    }

    #[test]
    fn scale_150_uses_scaled_cell_and_fewer_columns() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 150);
        assert_eq!((metrics.cell, metrics.gutter, metrics.margin), (156, 12, 24));
        assert_eq!((metrics.cols, metrics.rows), (3, 6));
        assert_eq!(metrics.pitch(), 168);
    }

    #[test]
    fn short_work_area_shrinks_cell_to_keep_min_rows() {
        let metrics = GridMetrics::for_work_area(work(1280, 560), 100);
        assert!(metrics.rows >= MIN_ROWS);
        assert!(metrics.cell < 104 && metrics.cell >= 80);
    }

    #[test]
    fn spans_are_cells_plus_gutters() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        assert_eq!(metrics.span_px(Span::SINGLE), (104, 104));
        assert_eq!(metrics.span_px(Span::WIDE), (216, 104));
        assert_eq!(metrics.span_px(Span::TALL), (104, 216));
        assert_eq!(metrics.span_px(Span::LARGE), (216, 216));
    }

    #[test]
    fn neighbouring_cells_are_separated_by_exactly_one_gutter() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let a = metrics.rect_of(Placement::left(0, 0, Span::SINGLE));
        let b = metrics.rect_of(Placement::left(1, 0, Span::SINGLE));
        let c = metrics.rect_of(Placement::left(0, 1, Span::SINGLE));
        assert_eq!(b.x - a.right(), 8);
        assert_eq!(c.y - a.bottom(), 8);
        assert_eq!((a.x, a.y), (16, 16));
    }

    #[test]
    fn right_placement_is_flush_with_right_margin() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let rect = metrics.rect_of(Placement { col: 0, row: 0, span: Span::LARGE, right: true });
        assert_eq!(rect.right(), 1920 - 16);
        assert_eq!(rect.width, 216);
    }

    #[test]
    fn snap_rounds_to_nearest_cell_and_clamps_to_grid() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        // Pitch 112; 16 + 112 + 40 px lands nearest column 1.
        let snapped = metrics.snap(Point::new(16 + 112 + 40, 16 + 112 * 2 + 10), Span::SINGLE, false);
        assert_eq!((snapped.col, snapped.row), (1, 2));
        let far = metrics.snap(Point::new(5000, 5000), Span::LARGE, false);
        assert_eq!((far.col, far.row), (3, 7));
    }

    #[test]
    fn adjacent_cells_do_not_collide_but_overlapping_ones_do() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let a = Placement::left(0, 0, Span::WIDE);
        assert!(!metrics.collides(a, Placement::left(2, 0, Span::SINGLE)));
        assert!(metrics.collides(a, Placement::left(1, 0, Span::SINGLE)));
        assert!(metrics.collides(a, Placement::left(1, 0, Span::TALL)));
    }

    #[test]
    fn first_free_is_row_major_and_skips_occupied_cells() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let occupied = [Placement::left(0, 0, Span::WIDE), Placement::left(2, 0, Span::SINGLE)];
        assert_eq!(
            metrics.first_free(Span::SINGLE, &occupied),
            Some(Placement::left(3, 0, Span::SINGLE))
        );
        let large = [Placement::left(0, 0, Span::LARGE)];
        let slot = metrics.first_free(Span::LARGE, &large).expect("free");
        assert_eq!((slot.col, slot.row), (2, 0));
    }

    #[test]
    fn nearest_free_pushes_past_collisions() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let occupied = [Placement::left(1, 1, Span::SINGLE)];
        let slot = metrics.nearest_free(Placement::left(1, 1, Span::SINGLE), &occupied).expect("free");
        // Both (1, 0) and (0, 1) are one cell away; the row-major tie-break picks (1, 0).
        assert_eq!((slot.col, slot.row), (1, 0));
    }

    #[test]
    fn reflow_moves_items_that_no_longer_fit() {
        let big = GridMetrics::for_work_area(work(1920, 1044), 100);
        let small = GridMetrics::for_work_area(work(1366, 732), 100);
        assert_eq!((big.cols, small.cols), (5, 3));
        let items = [Placement::left(4, 0, Span::SINGLE), Placement::left(0, 0, Span::SINGLE)];
        let placed = small.reflow(&items);
        assert_eq!(placed[1], Some(Placement::left(0, 0, Span::SINGLE)));
        let moved = placed[0].expect("moved");
        assert!(moved.fits(small));
        assert!(!small.collides(moved, Placement::left(0, 0, Span::SINGLE)));
    }

    #[test]
    fn right_anchored_widget_stays_flush_right_across_resolutions() {
        let big = GridMetrics::for_work_area(work(1920, 1044), 100);
        let small = GridMetrics::for_work_area(work(1366, 732), 100);
        let right = Placement { col: 0, row: 0, span: Span::SINGLE, right: true };
        let kept = small.reflow(&[right])[0].expect("fits");
        assert!(kept.right);
        assert_eq!(small.rect_of(kept).right(), 1366 - 16);
        assert_eq!(big.rect_of(right).right(), 1920 - 16);
    }

    #[test]
    fn migration_keeps_side_and_resolves_overlap() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let left = metrics.migrate(Rect::new(30, 40, 216, 216), Span::LARGE, &[]).expect("left");
        assert!(!left.right);
        assert_eq!((left.col, left.row), (0, 0));
        let right = metrics.migrate(Rect::new(1650, 40, 216, 216), Span::LARGE, &[]).expect("right");
        assert!(right.right);
        let blocked = [Placement::left(0, 0, Span::LARGE)];
        let moved = metrics.migrate(Rect::new(30, 40, 216, 216), Span::LARGE, &blocked).expect("moved");
        assert!(!metrics.collides(moved, blocked[0]));
    }

    #[test]
    fn default_layout_is_collision_free_and_fits_full_hd() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let layout = default_layout(metrics);
        let placed: Vec<Placement> = layout.iter().map(|(_, slot)| slot.expect("fits")).collect();
        for (index, (kind, slot)) in layout.iter().enumerate() {
            let slot = slot.expect("slot");
            assert_eq!(slot.span, default_span(*kind));
            assert!(slot.fits(metrics));
            for other in &placed[index + 1..] {
                assert!(!metrics.collides(slot, *other));
            }
        }
    }

    #[test]
    fn default_layout_reflows_on_small_grid() {
        let metrics = GridMetrics::for_work_area(work(1366, 732), 100);
        let layout = default_layout(metrics);
        let placed: Vec<Placement> = layout.iter().filter_map(|(_, slot)| *slot).collect();
        assert_eq!(placed.len(), layout.len());
        for (index, slot) in placed.iter().enumerate() {
            assert!(slot.fits(metrics));
            for other in &placed[index + 1..] {
                assert!(!metrics.collides(*slot, *other));
            }
        }
    }

    #[test]
    fn store_add_fills_first_free_cells_row_major() {
        use crate::widgets::{WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mut store = WidgetStore::default();
        store.add_on_grid(WidgetKind::Clock, metrics).expect("clock");
        store.add_on_grid(WidgetKind::Calendar, metrics).expect("calendar");
        store.add_on_grid(WidgetKind::Clock, metrics).expect("clock 2");
        assert_eq!(
            store.placements(),
            vec![
                Placement::left(0, 0, Span::SINGLE),
                Placement::left(1, 0, Span::LARGE),
                Placement::left(3, 0, Span::SINGLE),
            ]
        );
    }

    #[test]
    fn grid_layout_survives_serialize_round_trip() {
        use crate::widgets::{self, WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mut store = WidgetStore::default();
        store.add_on_grid(WidgetKind::Weather, metrics).expect("weather");
        store.add_on_grid(WidgetKind::Clock, metrics).expect("clock");
        let parsed = widgets::parse(&widgets::serialize(&store)).expect("parse");
        assert_eq!(parsed.placements(), store.placements());
        let rects: Vec<_> = parsed.widgets().iter().map(|w| w.rect).collect();
        let expected: Vec<_> = store.widgets().iter().map(|w| w.rect).collect();
        assert_eq!(rects, expected);
    }

    #[test]
    fn legacy_free_position_records_snap_to_nearest_free_cells() {
        use crate::widgets;
        // Two 9-field legacy records (no anchor, no grid) stacked on the same pixels.
        let text = "v1\n1\tclock\t30\t40\t340\t150\t0\t\t\n2\tclock\t32\t42\t340\t150\t0\t\t\n";
        let mut store = widgets::parse(text).expect("legacy parse");
        assert!(store.placements().is_empty());
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        store.layout_grid(metrics);
        let placed = store.placements();
        assert_eq!(placed.len(), 2);
        assert!(!metrics.collides(placed[0], placed[1]));
        assert_eq!(placed[0], Placement::left(0, 0, Span::SINGLE));
        assert_eq!(store.widgets()[0].rect, metrics.rect_of(placed[0]));
    }

    #[test]
    fn resize_on_grid_moves_to_free_cell_when_span_grows() {
        use crate::widgets::{WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mut store = WidgetStore::default();
        let clock = store.add_on_grid(WidgetKind::Clock, metrics).expect("clock");
        let other = store.add_on_grid(WidgetKind::Clock, metrics).expect("other");
        assert!(store.resize_on_grid(clock, Span::LARGE, metrics));
        let grown = store.get(clock).expect("clock").grid.expect("cell");
        assert_eq!(grown.span, Span::LARGE);
        assert!(!metrics.collides(grown, store.get(other).expect("other").grid.expect("cell")));
        assert_eq!(store.get(clock).expect("clock").rect.width, 216);
    }

    #[test]
    fn drop_snaps_to_cell_then_to_nearest_free_when_taken() {
        use crate::widgets::{WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mut store = WidgetStore::default();
        let first = store.add_on_grid(WidgetKind::Clock, metrics).expect("first");
        let second = store.add_on_grid(WidgetKind::Clock, metrics).expect("second");
        // Drop the second clock onto the first clock's cell: it must move to a free cell.
        assert!(store.drop_on_grid(second, Point::new(20, 20), metrics));
        let a = store.get(first).expect("a").grid.expect("cell");
        let b = store.get(second).expect("b").grid.expect("cell");
        assert_ne!(a, b);
        assert!(!metrics.collides(a, b));
        // A free drop lands on the snapped cell.
        assert!(store.drop_on_grid(second, Point::new(16 + 2 * 112, 16 + 3 * 112), metrics));
        assert_eq!(store.get(second).expect("b").grid.expect("cell").col, 2);
    }

    #[test]
    fn default_layout_store_has_one_widget_per_slot() {
        use crate::widgets::{WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let store = WidgetStore::with_default_layout(metrics);
        assert_eq!(store.widgets().len(), 9);
        assert!(store.widgets().iter().all(|widget| widget.inherits_surface()));
        assert_eq!(store.count_of(WidgetKind::Calendar), 1);
        let calendar = store.widgets().iter().find(|w| w.kind == WidgetKind::Calendar).expect("calendar");
        assert_eq!(calendar.rect.width, 216);
    }

    #[test]
    fn preview_matches_the_cell_a_drop_or_resize_lands_on() {
        use crate::widgets::{WidgetKind, WidgetStore};
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mut store = WidgetStore::default();
        let first = store.add_on_grid(WidgetKind::Clock, metrics).expect("first");
        let second = store.add_on_grid(WidgetKind::Clock, metrics).expect("second");
        // The drop point sits on the first clock's cell: the preview is the pushed cell.
        let origin = Point::new(20, 20);
        let preview = store.preview_drop(second, origin, metrics).expect("preview");
        assert!(!metrics.collides(preview, store.get(first).expect("a").grid.expect("cell")));
        assert!(store.drop_on_grid(second, origin, metrics));
        assert_eq!(store.get(second).expect("b").grid, Some(preview));

        let resize = store.preview_resize(second, Span::LARGE, metrics).expect("resize");
        assert_eq!(resize.span, Span::LARGE);
        assert!(store.resize_on_grid(second, Span::LARGE, metrics));
        assert_eq!(store.get(second).expect("b").grid.expect("cell").span, Span::LARGE);
    }

    #[test]
    fn span_for_px_picks_nearest_span() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        assert_eq!(metrics.span_for_px(104, 104), Span::SINGLE);
        assert_eq!(metrics.span_for_px(230, 110), Span::WIDE);
        assert_eq!(metrics.span_for_px(120, 240), Span::TALL);
        assert_eq!(metrics.span_for_px(400, 400), Span::LARGE);
    }

    #[test]
    fn span_labels_round_trip() {
        for span in Span::ALL {
            assert_eq!(Span::from_label(&span.label()), Some(span));
        }
        assert_eq!(Span::from_label("3x3"), None);
    }
}
