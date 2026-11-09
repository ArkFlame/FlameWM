//! Renderer-neutral stack/grid/form layout solver ported from the native Flame UI layer.

use flamewm_api::{Rect, Size};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    #[must_use]
    pub const fn all(value: i32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    #[must_use]
    pub const fn symmetric(horizontal: i32, vertical: i32) -> Self {
        Self {
            left: horizontal,
            top: vertical,
            right: horizontal,
            bottom: vertical,
        }
    }

    #[must_use]
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MeasureMode {
    Fixed,
    #[default]
    Content,
    Flex,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Alignment {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measure {
    pub mode: MeasureMode,
    pub value: i32,
    pub minimum: i32,
    /// `None` means unbounded.
    pub maximum: Option<i32>,
}

impl Measure {
    #[must_use]
    pub const fn fixed(value: i32) -> Self {
        Self {
            mode: MeasureMode::Fixed,
            value,
            minimum: 0,
            maximum: None,
        }
    }

    #[must_use]
    pub const fn flex(weight: i32) -> Self {
        Self {
            mode: MeasureMode::Flex,
            value: weight,
            minimum: 0,
            maximum: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutSpec {
    pub width: Measure,
    pub height: Measure,
    pub horizontal: Alignment,
    pub vertical: Alignment,
    pub visible: bool,
    pub content: Size,
}

impl Default for LayoutSpec {
    fn default() -> Self {
        Self {
            width: Measure::default(),
            height: Measure::default(),
            horizontal: Alignment::Stretch,
            vertical: Alignment::Stretch,
            visible: true,
            content: Size::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutItem {
    pub id: String,
    pub span: i32,
    pub spec: LayoutSpec,
}

impl LayoutItem {
    #[must_use]
    pub fn content(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            span: 1,
            spec: LayoutSpec::default(),
        }
    }

    #[must_use]
    pub fn fill(id: impl Into<String>) -> Self {
        let spec = LayoutSpec {
            width: Measure::flex(1),
            height: Measure::flex(1),
            ..LayoutSpec::default()
        };
        Self {
            id: id.into(),
            span: 1,
            spec,
        }
    }

    #[must_use]
    pub fn with_span(mut self, span: i32) -> Self {
        self.span = span;
        self
    }

    #[must_use]
    pub fn visible(mut self, visible: bool) -> Self {
        self.spec.visible = visible;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutResult {
    pub id: String,
    pub rect: Rect,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutOptions {
    pub gap: i32,
    pub padding: Insets,
    pub horizontal: Alignment,
    pub vertical: Alignment,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StackDirection {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stack {
    pub direction: StackDirection,
    pub items: Vec<LayoutItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grid {
    pub items: Vec<LayoutItem>,
}

#[must_use]
pub fn solve_stack(stack: &Stack, bounds: Rect, options: LayoutOptions) -> Vec<LayoutResult> {
    solve_linear(
        &stack.items,
        bounds,
        matches!(stack.direction, StackDirection::Horizontal),
        options,
    )
}

#[must_use]
pub fn solve_grid(
    grid: &Grid,
    bounds: Rect,
    columns: i32,
    options: LayoutOptions,
) -> Vec<LayoutResult> {
    if columns <= 0 {
        return Vec::new();
    }
    let inner_width = bounds
        .width
        .saturating_sub(options.padding.left)
        .saturating_sub(options.padding.right)
        .max(0);
    let inner_height = bounds
        .height
        .saturating_sub(options.padding.top)
        .saturating_sub(options.padding.bottom)
        .max(0);
    let cell_width =
        (inner_width.saturating_sub((columns - 1).saturating_mul(options.gap)) / columns).max(0);

    let mut positions = vec![None; grid.items.len()];
    let mut row_use = vec![0_i32];
    for (index, item) in grid.items.iter().enumerate() {
        if !item.spec.visible {
            continue;
        }
        let span = item.span.max(1).min(columns);
        let mut row = row_use.len() - 1;
        // span <= columns, so `columns - span` is exact; a saturating sum would accept a full row.
        while row_use[row] > columns - span {
            row += 1;
            if row == row_use.len() {
                row_use.push(0);
            }
        }
        let column = row_use[row];
        row_use[row] += span;
        positions[index] = Some((i32::try_from(row).unwrap_or(i32::MAX), column));
    }
    let rows = i32::try_from(row_use.len()).unwrap_or(i32::MAX);
    let cell_height = if rows == 0 {
        0
    } else {
        (inner_height.saturating_sub((rows - 1).saturating_mul(options.gap)) / rows).max(0)
    };

    grid.items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let Some((row, column)) = positions[index] else {
                return LayoutResult {
                    id: item.id.clone(),
                    rect: Rect::default(),
                    visible: false,
                };
            };
            let span = item.span.max(1).min(columns - column);
            let cell_x = bounds
                .x
                .saturating_add(options.padding.left)
                .saturating_add(column.saturating_mul(cell_width.saturating_add(options.gap)));
            let cell_y = bounds
                .y
                .saturating_add(options.padding.top)
                .saturating_add(row.saturating_mul(cell_height.saturating_add(options.gap)));
            let cell_extent = cell_width
                .saturating_mul(span)
                .saturating_add(options.gap.saturating_mul(span - 1));
            let horizontal = resolve_alignment(options.horizontal, item.spec.horizontal);
            let vertical = resolve_alignment(options.vertical, item.spec.vertical);
            let width = if horizontal == Alignment::Stretch {
                cell_extent
            } else {
                clamp_measure(basis(item, true), item.spec.width)
            };
            let height = if vertical == Alignment::Stretch {
                cell_height
            } else {
                clamp_measure(basis(item, false), item.spec.height)
            };
            LayoutResult {
                id: item.id.clone(),
                rect: Rect::new(
                    cell_x.saturating_add(aligned_offset(cell_extent, width, horizontal)),
                    cell_y.saturating_add(aligned_offset(cell_height, height, vertical)),
                    width,
                    height,
                ),
                visible: true,
            }
        })
        .collect()
}

fn solve_linear(
    items: &[LayoutItem],
    bounds: Rect,
    horizontal: bool,
    options: LayoutOptions,
) -> Vec<LayoutResult> {
    let origin = pick(
        horizontal,
        bounds.x.saturating_add(options.padding.left),
        bounds.y.saturating_add(options.padding.top),
    );
    let cross_origin = pick(
        horizontal,
        bounds.y.saturating_add(options.padding.top),
        bounds.x.saturating_add(options.padding.left),
    );
    let inner_width = bounds
        .width
        .saturating_sub(options.padding.left)
        .saturating_sub(options.padding.right);
    let inner_height = bounds
        .height
        .saturating_sub(options.padding.top)
        .saturating_sub(options.padding.bottom);
    let available = pick(horizontal, inner_width, inner_height).max(0);
    let cross = pick(horizontal, inner_height, inner_width).max(0);

    let mut sizes = vec![0_i32; items.len()];
    let mut used: i32 = 0;
    let mut flex_weight: i32 = 0;
    let mut visible_count: i32 = 0;
    for (index, item) in items.iter().enumerate() {
        if !item.spec.visible {
            continue;
        }
        sizes[index] = basis(item, horizontal);
        used = used.saturating_add(sizes[index]);
        let measure = pick(horizontal, item.spec.width, item.spec.height);
        if measure.mode == MeasureMode::Flex {
            flex_weight = flex_weight.saturating_add(measure.value.max(1));
        }
        visible_count += 1;
    }
    used = used.saturating_add((visible_count - 1).max(0).saturating_mul(options.gap));
    let extra = available.saturating_sub(used);
    if extra > 0 && flex_weight > 0 {
        for (index, item) in items.iter().enumerate() {
            if !item.spec.visible {
                continue;
            }
            let measure = pick(horizontal, item.spec.width, item.spec.height);
            if measure.mode == MeasureMode::Flex {
                // Widened: `extra * weight` can exceed i32 before the division by the weight sum.
                let part =
                    i64::from(extra) * i64::from(measure.value.max(1)) / i64::from(flex_weight);
                let part = i32::try_from(part).unwrap_or(i32::MAX);
                sizes[index] = clamp_measure(sizes[index].saturating_add(part), measure);
            }
        }
    }

    let content_extent = sizes
        .iter()
        .zip(items)
        .filter(|(_, item)| item.spec.visible)
        .fold(0_i32, |sum, (size, _)| sum.saturating_add(*size))
        .saturating_add((visible_count - 1).max(0).saturating_mul(options.gap));
    let main_alignment = pick(horizontal, options.horizontal, options.vertical);
    let mut cursor =
        origin.saturating_add(aligned_offset(available, content_extent, main_alignment));
    let mut result = Vec::with_capacity(items.len());

    for (index, item) in items.iter().enumerate() {
        if !item.spec.visible {
            result.push(LayoutResult {
                id: item.id.clone(),
                rect: Rect::default(),
                visible: false,
            });
            continue;
        }
        let cross_measure = pick(horizontal, item.spec.height, item.spec.width);
        let option_alignment = pick(horizontal, options.vertical, options.horizontal);
        let item_alignment = pick(horizontal, item.spec.vertical, item.spec.horizontal);
        let cross_alignment = resolve_alignment(option_alignment, item_alignment);
        let mut cross_size =
            if cross_measure.mode == MeasureMode::Flex || cross_alignment == Alignment::Stretch {
                cross
            } else {
                basis(item, !horizontal)
            };
        cross_size = clamp_measure(cross_size, cross_measure);
        if cross_alignment == Alignment::Stretch {
            cross_size = clamp_measure(cross, cross_measure);
        }
        let offset = aligned_offset(cross, cross_size, cross_alignment);
        let cross_at = cross_origin.saturating_add(offset);
        let rect = if horizontal {
            Rect::new(cursor, cross_at, sizes[index], cross_size)
        } else {
            Rect::new(cross_at, cursor, cross_size, sizes[index])
        };
        result.push(LayoutResult {
            id: item.id.clone(),
            rect,
            visible: true,
        });
        cursor = cursor.saturating_add(sizes[index].saturating_add(options.gap));
    }
    result
}

fn pick<T>(horizontal: bool, horizontal_value: T, vertical_value: T) -> T {
    if horizontal {
        horizontal_value
    } else {
        vertical_value
    }
}

/// Item alignment applies unless the container sets an explicit alignment.
fn resolve_alignment(container: Alignment, item: Alignment) -> Alignment {
    if container == Alignment::Stretch {
        item
    } else {
        container
    }
}

fn basis(item: &LayoutItem, horizontal: bool) -> i32 {
    let measure = pick(horizontal, item.spec.width, item.spec.height);
    let content = pick(
        horizontal,
        item.spec.content.width,
        item.spec.content.height,
    );
    match measure.mode {
        MeasureMode::Fixed | MeasureMode::Flex => clamp_measure(measure.value, measure),
        MeasureMode::Content => clamp_measure(content, measure),
    }
}

fn clamp_measure(value: i32, measure: Measure) -> i32 {
    let value = value.max(measure.minimum);
    measure
        .maximum
        .map_or(value, |maximum| value.min(maximum))
        .max(0)
}

fn aligned_offset(available: i32, size: i32, alignment: Alignment) -> i32 {
    match alignment {
        Alignment::Center => available.saturating_sub(size) / 2,
        Alignment::End => available.saturating_sub(size),
        Alignment::Start | Alignment::Stretch => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_flex_consumes_remaining_width() {
        let mut stack = Stack {
            direction: StackDirection::Horizontal,
            items: Vec::new(),
        };
        let mut fixed = LayoutItem::content("fixed");
        fixed.spec.width = Measure::fixed(40);
        fixed.spec.height = Measure::fixed(20);
        let mut flex = LayoutItem::fill("flex");
        flex.spec.height = Measure::fixed(20);
        stack.items.extend([fixed, flex]);
        let result = solve_stack(&stack, Rect::new(0, 0, 100, 20), LayoutOptions::default());
        assert_eq!(result[0].rect.width, 40);
        assert_eq!(result[1].rect.width, 60);
    }

    #[test]
    fn hidden_items_do_not_consume_gap_or_extent() {
        let stack = Stack {
            direction: StackDirection::Horizontal,
            items: vec![
                LayoutItem::fill("one"),
                LayoutItem::fill("two").visible(false),
            ],
        };
        let options = LayoutOptions {
            gap: 8,
            ..LayoutOptions::default()
        };
        let result = solve_stack(&stack, Rect::new(0, 0, 100, 20), options);
        assert_eq!(result[0].rect.width, 100);
        assert!(!result[1].visible);
    }

    #[test]
    fn grid_span_wraps_following_item_instead_of_overlapping() {
        let grid = Grid {
            items: vec![
                LayoutItem::content("a").with_span(4),
                LayoutItem::content("b"),
            ],
        };
        let result = solve_grid(&grid, Rect::new(0, 0, 100, 50), 2, LayoutOptions::default());
        assert_eq!(result[0].rect.width, 100);
        assert_eq!(result[1].rect.x, 0);
        assert!(result[1].rect.y > result[0].rect.y);
    }

    #[test]
    fn hidden_grid_items_do_not_consume_cells() {
        let grid = Grid {
            items: vec![
                LayoutItem::content("hidden").visible(false),
                LayoutItem::content("shown"),
            ],
        };
        let result = solve_grid(&grid, Rect::new(0, 0, 100, 40), 2, LayoutOptions::default());
        assert!(!result[0].visible);
        assert_eq!(result[1].rect.x, 0);
    }

    /// Malformed or huge geometry must never panic (debug builds trap i32 overflow).
    #[test]
    fn huge_grid_inputs_saturate_instead_of_overflowing() {
        let grid = Grid {
            items: vec![
                LayoutItem::content("a").with_span(i32::MAX),
                LayoutItem::fill("b"),
                LayoutItem::content("c").with_span(-4),
            ],
        };
        let options = LayoutOptions {
            gap: i32::MAX,
            padding: Insets::all(i32::MAX),
            ..LayoutOptions::default()
        };
        let result = solve_grid(
            &grid,
            Rect::new(i32::MIN, i32::MAX, i32::MAX, i32::MAX),
            3,
            options,
        );
        assert_eq!(result.len(), 3);
        assert!(
            result
                .iter()
                .all(|entry| entry.rect.width >= 0 && entry.rect.height >= 0)
        );
        let result = solve_grid(
            &grid,
            Rect::new(0, 0, i32::MAX, i32::MAX),
            i32::MAX,
            options,
        );
        assert!(
            result
                .iter()
                .all(|entry| entry.rect.width >= 0 && entry.rect.height >= 0)
        );
    }

    #[test]
    fn huge_linear_inputs_saturate_instead_of_overflowing() {
        let flex = |id: &str, weight: i32| LayoutItem {
            id: id.to_string(),
            span: 1,
            spec: LayoutSpec {
                width: Measure::flex(weight),
                height: Measure::flex(weight),
                ..LayoutSpec::default()
            },
        };
        let fixed = LayoutItem {
            id: "fixed".to_string(),
            span: 1,
            spec: LayoutSpec {
                width: Measure::fixed(i32::MAX),
                height: Measure::fixed(i32::MAX),
                ..LayoutSpec::default()
            },
        };
        let stack = Stack {
            direction: StackDirection::Horizontal,
            items: vec![
                fixed,
                flex("a", i32::MAX),
                flex("b", i32::MAX),
                flex("c", 1),
            ],
        };
        let options = LayoutOptions {
            gap: i32::MAX,
            padding: Insets::all(1),
            ..LayoutOptions::default()
        };
        let result = solve_stack(
            &stack,
            Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            options,
        );
        assert_eq!(result.len(), 4);
        assert!(
            result
                .iter()
                .all(|entry| entry.rect.width >= 0 && entry.rect.height >= 0)
        );
    }

    /// A flex share is `extra * weight / total`; the product must be exact past i32. Basis is the
    /// weight itself, so `extra` is what the two weights leave: `2e9 - (1 + 1e9) = 999_999_999`.
    #[test]
    fn flex_share_is_exact_when_product_exceeds_i32() {
        let flex = |id: &str, weight: i32| LayoutItem {
            id: id.to_string(),
            span: 1,
            spec: LayoutSpec {
                width: Measure::flex(weight),
                height: Measure::flex(1),
                ..LayoutSpec::default()
            },
        };
        let stack = Stack {
            direction: StackDirection::Horizontal,
            items: vec![flex("a", 1), flex("b", 1_000_000_000)],
        };
        let result = solve_stack(
            &stack,
            Rect::new(0, 0, 2_000_000_000, 10),
            LayoutOptions::default(),
        );
        // a = 1 + 999_999_999 * 1 / 1_000_000_001 = 1; b = 1e9 + 999_999_999 * 1e9 / 1_000_000_001.
        assert_eq!(result[0].rect.width, 1);
        assert_eq!(result[1].rect.x, 1);
        assert_eq!(result[1].rect.width, 1_999_999_998);
    }
}
