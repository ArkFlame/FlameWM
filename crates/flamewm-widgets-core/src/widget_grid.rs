//! Slot grid for desktop widgets: full-area cell metrics, spans, per-kind sizes,
//! snapping, collisions, reflow and migration. Pure math only; the widget store and
//! the desktop UI call these functions.
//!
//! The grid always covers the usable work area edge to edge inside the margin. The
//! column and row counts come from the target cell size; the lanes then stretch to
//! fill the remainder exactly, so `margin + cells + gutters + margin == work size` on
//! both axes. Placements are cells counted from the left and top margins; there is no
//! side anchoring in the grid itself.

use flamewm_api::{Point, Rect};

use crate::widgets::WidgetKind;

/// Target cell edge in logical pixels at 100% scale. Counts are rounded to the nearest
/// whole number, so the stretched cells stay close to this size.
pub const TARGET_CELL: i32 = 104;
/// Uniform gutter between cells, in logical pixels.
pub const GUTTER: i32 = 8;
/// Distance from the work-area edge to the outer cells, in logical pixels.
pub const MARGIN: i32 = 16;

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
    /// Custom-only sizes: three cells wide and four cells wide, two tall.
    pub const WIDE3: Self = Self { w: 3, h: 2 };
    pub const WIDE4: Self = Self { w: 4, h: 2 };
    pub const ALL: [Self; 6] = [
        Self::SINGLE,
        Self::WIDE,
        Self::TALL,
        Self::LARGE,
        Self::WIDE3,
        Self::WIDE4,
    ];

    #[must_use]
    pub fn label(self) -> String {
        format!("{}x{}", self.w, self.h)
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|span| span.label() == label)
    }
}

/// Default size of each widget kind. Add a kind here and in `allowed_spans`.
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

/// Sizes a kind may take. Every `default_span` must appear here.
#[must_use]
pub const fn allowed_spans(kind: WidgetKind) -> &'static [Span] {
    match kind {
        WidgetKind::Clock
        | WidgetKind::Cpu
        | WidgetKind::Memory
        | WidgetKind::History
        | WidgetKind::Network
        | WidgetKind::Ring => &[Span::SINGLE, Span::WIDE, Span::LARGE],
        WidgetKind::Custom => &[
            Span::SINGLE,
            Span::WIDE,
            Span::LARGE,
            Span::WIDE3,
            Span::WIDE4,
        ],
        WidgetKind::Media => &[Span::WIDE, Span::LARGE],
        WidgetKind::Weather | WidgetKind::Calendar => &[Span::WIDE, Span::LARGE],
    }
}

/// The allowed size closest to `requested` (Manhattan in cells, first listed on ties).
#[must_use]
pub fn fit_span(kind: WidgetKind, requested: Span) -> Span {
    allowed_spans(kind)
        .iter()
        .copied()
        .min_by_key(|span| (span.w - requested.w).abs() + (span.h - requested.h).abs())
        .unwrap_or_else(|| default_span(kind))
}

/// The edges a border drag moves. Opposite edges stay fixed while a border moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    /// Edge or corner named by its compass direction: `n`, `s`, `e`, `w`, `ne`, `nw`, `se`, `sw`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let (left, right, top, bottom) = match name {
            "n" => (false, false, true, false),
            "s" => (false, false, false, true),
            "e" => (false, true, false, false),
            "w" => (true, false, false, false),
            "ne" => (false, true, true, false),
            "nw" => (true, false, true, false),
            "se" => (false, true, false, true),
            "sw" => (true, false, false, true),
            _ => return None,
        };
        Some(Self {
            left,
            right,
            top,
            bottom,
        })
    }
}

/// Placement after dragging `edges` of `current` to the pixel rectangle `rect`. Opposite
/// edges stay put, the span is the allowed size nearest the dragged extent, and a moved
/// left or top edge moves the anchor cell so the opposite edge stays fixed.
#[must_use]
pub fn edge_resize(
    kind: WidgetKind,
    current: Placement,
    edges: Edges,
    rect: Rect,
    metrics: GridMetrics,
) -> Placement {
    let current = metrics.from_right(current);
    // Left/top boundaries sit at `origin + k * pitch`; right/bottom boundaries at
    // `origin + k * pitch - gutter`, so the gutter is added before rounding.
    let left_index =
        |px: i32, origin: i32, pitch: i32| (px - origin + pitch / 2).div_euclid(pitch.max(1));
    let right_index = |px: i32, origin: i32, pitch: i32| {
        (px - origin + metrics.gutter + pitch / 2).div_euclid(pitch.max(1))
    };
    let x0 = metrics.work.x + metrics.margin;
    let y0 = metrics.work.y + metrics.margin;
    let right_fixed = current.col + current.span.w;
    let bottom_fixed = current.row + current.span.h;
    let (col, width) = if edges.left && !edges.right {
        let left = left_index(rect.x, x0, metrics.pitch_x);
        (left, right_fixed - left)
    } else if edges.right && !edges.left {
        (
            current.col,
            right_index(rect.right(), x0, metrics.pitch_x) - current.col,
        )
    } else {
        (current.col, current.span.w)
    };
    let (row, height) = if edges.top && !edges.bottom {
        let top = left_index(rect.y, y0, metrics.pitch_y);
        (top, bottom_fixed - top)
    } else if edges.bottom && !edges.top {
        (
            current.row,
            right_index(rect.bottom(), y0, metrics.pitch_y) - current.row,
        )
    } else {
        (current.row, current.span.h)
    };
    let span = fit_span(
        kind,
        Span {
            w: width.max(1),
            h: height.max(1),
        },
    );
    let col = if edges.left && !edges.right {
        right_fixed - span.w
    } else {
        col
    };
    let row = if edges.top && !edges.bottom {
        bottom_fixed - span.h
    } else {
        row
    };
    Placement::left(
        col.clamp(0, (metrics.cols - span.w).max(0)),
        row.clamp(0, (metrics.rows - span.h).max(0)),
        span,
    )
}

/// A widget's cell position, counted from the left and top margins.
///
/// `right` is only set while reading a legacy record that anchored the widget to the
/// right edge; `GridMetrics::from_right` converts it to a left column before use.
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
        Self {
            col,
            row,
            span,
            right: false,
        }
    }

    /// Whether the cells stay inside the grid.
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

/// Grid geometry for one work area and one scale factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridMetrics {
    pub work: Rect,
    /// Gutter in device pixels.
    pub gutter: i32,
    /// Margin from the work-area edge in device pixels.
    pub margin: i32,
    pub cols: i32,
    pub rows: i32,
    /// Nominal column pitch (cell width + gutter) in device pixels.
    pub pitch_x: i32,
    /// Nominal row pitch (cell height + gutter) in device pixels.
    pub pitch_y: i32,
}

fn scale_px(logical: i32, scale_percent: i32) -> i32 {
    (logical * scale_percent + 50) / 100
}

/// Nearest whole number of pitches that fit in `len` between the margins.
fn count_for(len: i32, margin: i32, gutter: i32, pitch: i32) -> i32 {
    let usable = (len - 2 * margin + gutter).max(0);
    ((usable + pitch / 2) / pitch).max(1)
}

/// Start of lane `index` of `count` lanes that exactly fill `start..start+len`:
/// lane edges are `margin + index * (usable + gutter) / count`, rounded.
fn lane(start: i32, len: i32, margin: i32, gutter: i32, count: i32, index: i32) -> i32 {
    let total = len - 2 * margin + gutter;
    start + margin + (index * total + count / 2) / count
}

/// Round-trip of a value between two axis ranges, clamped to the new range.
fn rescale_axis(value: i32, old_range: i32, new_range: i32) -> i32 {
    let new_range = new_range.max(0);
    if old_range <= 0 {
        return value.clamp(0, new_range);
    }
    let scaled = (i64::from(value) * i64::from(new_range) * 2 + i64::from(old_range))
        / (2 * i64::from(old_range));
    (scaled as i32).clamp(0, new_range)
}

impl GridMetrics {
    /// Builds the grid for `work` (device pixels) at `scale_percent` (100 = 1x).
    #[must_use]
    pub fn for_work_area(work: Rect, scale_percent: i32) -> Self {
        let scale = scale_percent.clamp(50, 400);
        let gutter = scale_px(GUTTER, scale);
        let margin = scale_px(MARGIN, scale);
        let pitch = scale_px(TARGET_CELL, scale) + gutter;
        let cols = count_for(work.width, margin, gutter, pitch);
        let rows = count_for(work.height, margin, gutter, pitch);
        let total_x = (work.width - 2 * margin + gutter).max(0);
        let total_y = (work.height - 2 * margin + gutter).max(0);
        Self {
            work,
            gutter,
            margin,
            cols,
            rows,
            pitch_x: total_x / cols,
            pitch_y: total_y / rows,
        }
    }

    /// Nominal cell size in device pixels (before lane rounding).
    #[must_use]
    pub const fn cell_size(self) -> (i32, i32) {
        (self.pitch_x - self.gutter, self.pitch_y - self.gutter)
    }

    /// Centre of the work area, where new widgets appear.
    #[must_use]
    pub const fn center(self) -> Point {
        Point::new(
            self.work.x + self.work.width / 2,
            self.work.y + self.work.height / 2,
        )
    }

    /// Nominal pixel size of a span: `n` cells plus `n - 1` gutters per axis.
    #[must_use]
    pub const fn span_px(self, span: Span) -> (i32, i32) {
        let (cell_w, cell_h) = self.cell_size();
        (
            span.w * cell_w + (span.w - 1) * self.gutter,
            span.h * cell_h + (span.h - 1) * self.gutter,
        )
    }

    /// Pixel rectangle of a placement. Outer edges sit exactly on the margins.
    #[must_use]
    pub fn rect_of(self, placement: Placement) -> Rect {
        let x0 = lane(
            self.work.x,
            self.work.width,
            self.margin,
            self.gutter,
            self.cols,
            placement.col,
        );
        let x1 = lane(
            self.work.x,
            self.work.width,
            self.margin,
            self.gutter,
            self.cols,
            placement.col + placement.span.w,
        ) - self.gutter;
        let y0 = lane(
            self.work.y,
            self.work.height,
            self.margin,
            self.gutter,
            self.rows,
            placement.row,
        );
        let y1 = lane(
            self.work.y,
            self.work.height,
            self.margin,
            self.gutter,
            self.rows,
            placement.row + placement.span.h,
        ) - self.gutter;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Span whose pixel size is closest to `width` x `height`.
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

    /// Cell whose top-left is nearest to `point` (not checked for collisions).
    #[must_use]
    pub fn snap(self, point: Point, span: Span) -> Placement {
        let col = ((point.x - (self.work.x + self.margin)) + self.pitch_x / 2)
            .div_euclid(self.pitch_x.max(1))
            .min(self.cols - span.w)
            .max(0);
        let row = ((point.y - (self.work.y + self.margin)) + self.pitch_y / 2)
            .div_euclid(self.pitch_y.max(1))
            .min(self.rows - span.h)
            .max(0);
        Placement::left(col, row, span)
    }

    /// Whether two placements overlap in pixels. Gutters keep neighbours apart.
    #[must_use]
    pub fn collides(self, a: Placement, b: Placement) -> bool {
        let ra = self.rect_of(a);
        let rb = self.rect_of(b);
        ra.x < rb.right() && rb.x < ra.right() && ra.y < rb.bottom() && rb.y < ra.bottom()
    }

    /// Fits the grid and overlaps none of `occupied`.
    #[must_use]
    pub fn is_free(self, placement: Placement, occupied: &[Placement]) -> bool {
        placement.fits(self)
            && occupied
                .iter()
                .all(|other| !self.collides(placement, *other))
    }

    /// Converts a legacy right-anchored cell to a left column in this grid.
    #[must_use]
    pub const fn from_right(self, placement: Placement) -> Placement {
        if placement.right {
            Placement {
                col: self.cols - placement.col - placement.span.w,
                right: false,
                ..placement
            }
        } else {
            placement
        }
    }

    fn candidates(self, span: Span) -> impl Iterator<Item = Placement> {
        let cols = (self.cols - span.w).max(-1);
        let rows = (self.rows - span.h).max(-1);
        (0..=rows).flat_map(move |row| (0..=cols).map(move |col| Placement::left(col, row, span)))
    }

    /// Free cell closest (Manhattan distance in cells) to `want`; ties row-major.
    #[must_use]
    pub fn nearest_free(self, want: Placement, occupied: &[Placement]) -> Option<Placement> {
        self.candidates(want.span)
            .filter(|candidate| {
                occupied
                    .iter()
                    .all(|other| !self.collides(*candidate, *other))
            })
            .min_by_key(|candidate| {
                (
                    (candidate.col - want.col).abs() + (candidate.row - want.row).abs(),
                    candidate.row,
                    candidate.col,
                )
            })
    }

    /// Free cell whose centre is nearest to `point`; ties row-major.
    #[must_use]
    pub fn nearest_free_to(
        self,
        span: Span,
        point: Point,
        occupied: &[Placement],
    ) -> Option<Placement> {
        self.candidates(span)
            .filter(|candidate| {
                occupied
                    .iter()
                    .all(|other| !self.collides(*candidate, *other))
            })
            .min_by_key(|candidate| {
                let rect = self.rect_of(*candidate);
                let dx = i64::from(rect.x + rect.width / 2 - point.x);
                let dy = i64::from(rect.y + rect.height / 2 - point.y);
                (dx * dx + dy * dy, candidate.row, candidate.col)
            })
    }

    /// Re-places items after the grid changed. `from` is the previous grid size; when
    /// known, each item keeps its relative position on the new grid (proportional
    /// reflow). Items that no longer fit or overlap an earlier item move to the nearest
    /// free cell; `None` means the grid has no room left for that item.
    #[must_use]
    pub fn reflow(self, items: &[Placement], from: Option<(i32, i32)>) -> Vec<Option<Placement>> {
        let mut placed: Vec<Placement> = Vec::with_capacity(items.len());
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let scaled = match from {
                Some((old_cols, old_rows)) => Placement {
                    col: rescale_axis(item.col, old_cols - item.span.w, self.cols - item.span.w),
                    row: rescale_axis(item.row, old_rows - item.span.h, self.rows - item.span.h),
                    ..*item
                },
                None => *item,
            };
            let slot = if self.is_free(scaled, &placed) {
                Some(scaled)
            } else {
                self.nearest_free(scaled, &placed)
            };
            if let Some(slot) = slot {
                placed.push(slot);
            }
            out.push(slot);
        }
        out
    }

    /// Cell for a widget with a legacy pixel rect and no grid cell. Right-half rects
    /// map to the right-hand columns, as the legacy edge anchoring did.
    #[must_use]
    pub fn migrate(self, rect: Rect, span: Span, occupied: &[Placement]) -> Option<Placement> {
        let right = rect.x + rect.width / 2 > self.work.x + self.work.width / 2;
        if right {
            // Distance from the right edge, counted as a column from the right margin.
            let from_right = Point::new(self.work.x + self.work.right() - rect.right(), rect.y);
            let mirrored = self.snap(from_right, span);
            let want = Placement::left(self.cols - span.w - mirrored.col, mirrored.row, span);
            return self.nearest_free(want, occupied);
        }
        let want = self.snap(rect.origin(), span);
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

/// Places the default arrangement on `metrics`. Slots that no longer fit move to the
/// nearest free cell, and `None` means the grid is full.
#[must_use]
pub fn default_layout(metrics: GridMetrics) -> Vec<(WidgetKind, Option<Placement>)> {
    let mut placed: Vec<Placement> = Vec::with_capacity(DEFAULT_SLOTS.len());
    let mut out = Vec::with_capacity(DEFAULT_SLOTS.len());
    for (kind, col, row) in DEFAULT_SLOTS {
        let want = Placement::left(col, row, default_span(kind));
        let slot = if metrics.is_free(want, &placed) {
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

    /// Asserts the cells and gutters fill the work area exactly inside the margins.
    fn assert_fills(metrics: GridMetrics) {
        let first = metrics.rect_of(Placement::left(0, 0, Span::SINGLE));
        let last = metrics.rect_of(Placement::left(
            metrics.cols - 1,
            metrics.rows - 1,
            Span::SINGLE,
        ));
        let work = metrics.work;
        assert_eq!(first.x, work.x + metrics.margin, "left edge");
        assert_eq!(first.y, work.y + metrics.margin, "top edge");
        assert_eq!(last.right(), work.right() - metrics.margin, "right edge");
        assert_eq!(last.bottom(), work.bottom() - metrics.margin, "bottom edge");
        // sum(cells) + sum(gutters) + 2 margins == work size, on both axes.
        let cells_x: i32 = (0..metrics.cols)
            .map(|c| metrics.rect_of(Placement::left(c, 0, Span::SINGLE)).width)
            .sum();
        let cells_y: i32 = (0..metrics.rows)
            .map(|r| metrics.rect_of(Placement::left(0, r, Span::SINGLE)).height)
            .sum();
        assert_eq!(
            cells_x + (metrics.cols - 1) * metrics.gutter + 2 * metrics.margin,
            work.width
        );
        assert_eq!(
            cells_y + (metrics.rows - 1) * metrics.gutter + 2 * metrics.margin,
            work.height
        );
    }

    #[test]
    fn full_hd_at_100_percent_covers_the_whole_width() {
        // 1080 minus a 36 px taskbar strut.
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        assert_eq!((metrics.cols, metrics.rows), (17, 9));
        assert_eq!(metrics.gutter, 8);
        assert_eq!(metrics.margin, 16);
        assert_fills(metrics);
    }

    #[test]
    fn full_hd_without_strut_keeps_nine_rows() {
        let metrics = GridMetrics::for_work_area(work(1920, 1080), 100);
        assert_eq!((metrics.cols, metrics.rows), (17, 9));
        assert_fills(metrics);
    }

    #[test]
    fn hd_laptop_gives_twelve_by_six_and_fills() {
        let metrics = GridMetrics::for_work_area(work(1366, 732), 100);
        assert_eq!((metrics.cols, metrics.rows), (12, 6));
        assert_fills(metrics);
    }

    #[test]
    fn qhd_fills_with_scaled_grid() {
        let metrics = GridMetrics::for_work_area(work(2560, 1404), 100);
        assert_eq!((metrics.cols, metrics.rows), (23, 12));
        assert_fills(metrics);
    }

    #[test]
    fn scale_150_uses_scaled_gutter_and_margin() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 150);
        assert_eq!((metrics.gutter, metrics.margin), (12, 24));
        assert_eq!((metrics.cols, metrics.rows), (11, 6));
        assert_fills(metrics);
    }

    #[test]
    fn fill_holds_across_sizes_scales_and_taskbar_heights() {
        for &(w, h) in &[
            (1280, 720),
            (1366, 768),
            (1600, 900),
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
        ] {
            for &strut in &[0, 36, 48, 72] {
                for &scale in &[100, 125, 150, 200] {
                    assert_fills(GridMetrics::for_work_area(work(w, h - strut), scale));
                }
            }
        }
    }

    #[test]
    fn centre_cells_are_reachable_and_inside() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let mid = metrics.rect_of(Placement::left(
            metrics.cols / 2,
            metrics.rows / 2,
            Span::SINGLE,
        ));
        assert!(mid.x > 800 && mid.x < 1100, "centre column at {}", mid.x);
    }

    #[test]
    fn snap_and_nearest_free_to_centre_work() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let target = metrics.rect_of(Placement::left(8, 4, Span::SINGLE));
        assert_eq!(
            metrics.snap(target.origin(), Span::SINGLE),
            Placement::left(8, 4, Span::SINGLE)
        );
        let occupied = [Placement::left(8, 4, Span::SINGLE)];
        let free = metrics
            .nearest_free_to(Span::SINGLE, metrics.center(), &occupied)
            .unwrap();
        assert_ne!(free, occupied[0]);
        assert!(metrics.is_free(free, &occupied));
    }

    #[test]
    fn collisions_are_resolved_by_nearest_free_cell() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let occupied = [Placement::left(3, 3, Span::SINGLE)];
        let slot = metrics
            .nearest_free(Placement::left(3, 3, Span::SINGLE), &occupied)
            .unwrap();
        assert_eq!((slot.col - 3).abs() + (slot.row - 3).abs(), 1);
    }

    #[test]
    fn reflow_keeps_relative_position_when_grid_shrinks() {
        let big = GridMetrics::for_work_area(work(1920, 1044), 100);
        let small = GridMetrics::for_work_area(work(1366, 732), 100);
        let items = [Placement::left(16 - 2, 0, Span::SINGLE)]; // right-most column on 17
        let slots = small.reflow(&items, Some((big.cols, big.rows)));
        let slot = slots[0].unwrap();
        assert!(slot.fits(small));
        assert_eq!(
            slot.col,
            small.cols - 1 - 1,
            "right side stays on the right"
        );
    }

    #[test]
    fn reflow_without_previous_size_keeps_cells_that_still_fit() {
        let metrics = GridMetrics::for_work_area(work(1366, 732), 100);
        let items = [
            Placement::left(1, 1, Span::LARGE),
            Placement::left(11, 5, Span::SINGLE),
        ];
        let slots = metrics.reflow(&items, None);
        assert_eq!(slots[0], Some(Placement::left(1, 1, Span::LARGE)));
        assert_eq!(slots[1], Some(Placement::left(11, 5, Span::SINGLE)));
    }

    #[test]
    fn reflow_moves_overlapping_items_to_free_cells() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let items = [
            Placement::left(0, 0, Span::LARGE),
            Placement::left(1, 1, Span::SINGLE),
        ];
        let slots = metrics.reflow(&items, None);
        let a = slots[0].unwrap();
        let b = slots[1].unwrap();
        assert!(!metrics.collides(a, b));
    }

    #[test]
    fn right_anchored_legacy_cells_map_to_right_columns() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let legacy = Placement {
            col: 0,
            row: 0,
            span: Span::WIDE,
            right: true,
        };
        let left = metrics.from_right(legacy);
        assert_eq!(left.col, metrics.cols - 2);
        assert!(!left.right);
        assert_eq!(
            metrics.rect_of(left).right(),
            metrics.work.right() - metrics.margin
        );
    }

    #[test]
    fn migrate_maps_right_half_pixel_rects_to_right_columns() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let rect = Rect::new(1920 - 16 - 104, 16, 104, 104);
        let slot = metrics.migrate(rect, Span::SINGLE, &[]).unwrap();
        assert_eq!(slot.col, metrics.cols - 1);
    }

    #[test]
    fn span_sizes_are_allowed_per_kind() {
        assert_eq!(fit_span(WidgetKind::Clock, Span::TALL), Span::SINGLE);
        assert_eq!(fit_span(WidgetKind::Media, Span::SINGLE), Span::WIDE);
        assert_eq!(fit_span(WidgetKind::Weather, Span::SINGLE), Span::WIDE);
        assert_eq!(fit_span(WidgetKind::Clock, Span::LARGE), Span::LARGE);
    }

    #[test]
    fn every_default_span_is_an_allowed_span() {
        for kind in WidgetKind::ALL {
            assert!(
                allowed_spans(kind).contains(&default_span(kind)),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn custom_kind_allows_three_and_four_wide_sizes_only_for_custom() {
        assert_eq!(Span::from_label("3x2"), Some(Span::WIDE3));
        assert_eq!(Span::from_label("4x2"), Some(Span::WIDE4));
        assert!(allowed_spans(WidgetKind::Custom).contains(&Span::WIDE4));
        assert!(allowed_spans(WidgetKind::Custom).contains(&Span::WIDE3));
        assert_eq!(
            fit_span(WidgetKind::Custom, Span { w: 4, h: 2 }),
            Span::WIDE4
        );
        assert_eq!(fit_span(WidgetKind::Clock, Span::WIDE4), Span::LARGE);
        assert!(!allowed_spans(WidgetKind::Clock).contains(&Span::WIDE3));
    }

    #[test]
    fn default_layout_places_every_kind_on_a_full_grid() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let layout = default_layout(metrics);
        assert!(layout.iter().all(|(_, slot)| slot.is_some()));
        let cells: Vec<Placement> = layout.iter().filter_map(|(_, slot)| *slot).collect();
        for (i, a) in cells.iter().enumerate() {
            for b in &cells[i + 1..] {
                assert!(!metrics.collides(*a, *b));
            }
        }
    }

    #[test]
    fn span_for_px_picks_the_closest_span() {
        let metrics = GridMetrics::for_work_area(work(1920, 1044), 100);
        let (w, h) = metrics.span_px(Span::LARGE);
        assert_eq!(metrics.span_for_px(w, h), Span::LARGE);
        let (w, h) = metrics.span_px(Span::WIDE);
        assert_eq!(metrics.span_for_px(w, h), Span::WIDE);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn metrics() -> GridMetrics {
        GridMetrics::for_work_area(Rect::new(0, 0, 1920, 1080), 100)
    }

    fn edges(name: &str) -> Edges {
        Edges::from_name(name).expect("edge name")
    }

    #[test]
    fn names_map_to_edges_and_corners() {
        assert_eq!(
            edges("n"),
            Edges {
                left: false,
                right: false,
                top: true,
                bottom: false
            }
        );
        assert_eq!(
            edges("se"),
            Edges {
                left: false,
                right: true,
                top: false,
                bottom: true
            }
        );
        assert_eq!(
            edges("sw"),
            Edges {
                left: true,
                right: false,
                top: false,
                bottom: true
            }
        );
        assert!(Edges::from_name("x").is_none());
    }

    #[test]
    fn right_edge_drag_widens_to_two_cells() {
        let m = metrics();
        let current = Placement::left(0, 0, Span::SINGLE);
        let rect = m.rect_of(Placement::left(0, 0, Span::WIDE));
        assert_eq!(
            edge_resize(WidgetKind::Clock, current, edges("e"), rect, m),
            Placement::left(0, 0, Span::WIDE)
        );
    }

    #[test]
    fn left_edge_drag_moves_anchor_and_keeps_right_edge() {
        let m = metrics();
        // Custom 3x2 at cols 3..5; drag left edge to col 1 gives 5 cols -> WIDE4 anchored at col 2.
        let current = Placement::left(3, 0, Span::WIDE3);
        let rect = m.rect_of(Placement {
            span: Span { w: 5, h: 2 },
            ..Placement::left(1, 0, Span::SINGLE)
        });
        let got = edge_resize(WidgetKind::Custom, current, edges("w"), rect, m);
        assert_eq!(got, Placement::left(2, 0, Span::WIDE4));
        assert_eq!(got.col + got.span.w, 6, "right edge stays fixed");
    }

    #[test]
    fn top_edge_drag_moves_row_and_keeps_bottom_edge() {
        let m = metrics();
        let current = Placement::left(0, 4, Span::LARGE);
        let rect = m.rect_of(Placement {
            span: Span { w: 1, h: 3 },
            ..Placement::left(0, 3, Span::SINGLE)
        });
        let got = edge_resize(WidgetKind::Custom, current, edges("n"), rect, m);
        assert_eq!(got.row + got.span.h, 6, "bottom edge stays fixed");
        assert_eq!(
            got.span.h, 2,
            "3 rows requested, nearest allowed height is 2"
        );
    }

    #[test]
    fn right_edge_drag_past_grid_clamps_inside_grid() {
        let m = metrics();
        let last = m.cols - 1;
        let current = Placement::left(last, 0, Span::SINGLE);
        // Dragged two columns past the last one: wants WIDE at `last`, which must clamp to `last - 1`.
        let rect = m.rect_of(Placement {
            span: Span { w: 2, h: 1 },
            ..current
        });
        let got = edge_resize(WidgetKind::Clock, current, edges("e"), rect, m);
        assert!(got.fits(m), "placement stays inside the grid: {got:?}");
        assert_eq!(got.span, Span::WIDE, "Clock is limited to 2x1 here");
        assert_eq!(got.col, last - 1);
    }

    #[test]
    fn far_corner_drag_clamps_clock_to_large() {
        let m = metrics();
        let current = Placement::left(0, 0, Span::SINGLE);
        let rect = m.rect_of(Placement {
            span: Span {
                w: m.cols,
                h: m.rows,
            },
            ..current
        });
        let got = edge_resize(WidgetKind::Clock, current, edges("se"), rect, m);
        assert_eq!(got, Placement::left(0, 0, Span::LARGE));
    }
}
