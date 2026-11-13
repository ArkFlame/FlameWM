//! Layout engine: per-pass node tree walk, measure memo and box placement.

use super::*;

/// Node and available-size bits -> measured size.
pub(super) type MeasureMemo = crate::hash::FastMap<(u32, u32, u32), (f32, f32)>;

pub(super) struct Engine<'a> {
    pub(super) document: &'a RuntimeDocument,
    pub(super) interaction: InteractionState,
    pub(super) boxes: &'a mut [LayoutBox],
    /// `measure_node` results for this pass, keyed by node and the exact available size bits.
    /// Measuring reads only the document, so a node measured twice under one constraint (by its
    /// parent's measure and again by its own layout) returns the stored size.
    pub(super) measures: std::cell::RefCell<MeasureMemo>,
}

impl<'a> Engine<'a> {
    pub(super) fn style(&self, index: u32) -> std::borrow::Cow<'a, Style> {
        self.document.runtime_style_cow(index, self.interaction)
    }

    /// Sibling-link walk in document order. Borrows the document (`'a`), not
    /// `self`, so callers may mutate boxes while iterating.
    pub(super) fn children(&self, index: u32) -> impl Iterator<Item = u32> + 'a {
        let document: &'a RuntimeDocument = self.document;
        let nodes = &document.document.nodes;
        let mut cursor = nodes[index as usize].first_child;
        std::iter::from_fn(move || {
            let child = cursor?;
            cursor = nodes[child as usize].next_sibling;
            Some(child)
        })
    }

    pub(super) fn measure_node(&self, index: u32, avail_w: f32, avail_h: f32) -> (f32, f32) {
        let key = (index, avail_w.to_bits(), avail_h.to_bits());
        if let Some(&size) = self.measures.borrow().get(&key) {
            return size;
        }
        let size = self.measure_node_uncached(index, avail_w, avail_h);
        self.measures.borrow_mut().insert(key, size);
        size
    }

    pub(super) fn measure_node_uncached(
        &self,
        index: u32,
        avail_w: f32,
        avail_h: f32,
    ) -> (f32, f32) {
        let node = &self.document.document.nodes[index as usize];
        let style = self.style(index);
        if style.display == Display::None {
            return (0.0, 0.0);
        }
        if node.kind == NodeKind::Text {
            let text = self.document.text_for(index);
            let laid = crate::text_layout::layout_styled(
                text,
                style.font_size,
                style.font_weight,
                avail_w,
                style.text_wrap,
                style.break_anywhere,
                style.text_overflow,
            );
            return (
                clamp_length(laid.width, style.min_width, style.max_width, avail_w),
                clamp_length(laid.height, style.min_height, style.max_height, avail_h),
            );
        }
        if node.kind == NodeKind::Image {
            let asset = node
                .image
                .and_then(|asset| self.document.document.assets.get(asset as usize));
            let natural_w = asset.map(|asset| asset.width as f32).unwrap_or(0.0);
            let natural_h = asset.map(|asset| asset.height as f32).unwrap_or(0.0);
            let width = style.width.resolve(avail_w).unwrap_or(natural_w);
            let height = style.height.resolve(avail_h).unwrap_or(natural_h);
            return (
                clamp_length(width, style.min_width, style.max_width, avail_w),
                clamp_length(height, style.min_height, style.max_height, avail_h),
            );
        }

        let explicit_w = style.width.resolve(avail_w);
        let explicit_h = style.height.resolve(avail_h);
        // Children wrap inside the box as painted: its width is clamped by min/max width first.
        let box_w = clamp_length(
            explicit_w.unwrap_or(avail_w),
            style.min_width,
            style.max_width,
            avail_w,
        );
        let inner_w = box_w.max(style.padding.horizontal()) - style.padding.horizontal();
        let inner_h =
            explicit_h.unwrap_or(avail_h).max(style.padding.vertical()) - style.padding.vertical();
        let children = self.children(index);
        let mut content_w: f32 = 0.0;
        let mut content_h: f32 = 0.0;
        let mut flow_count = 0usize;

        match style.display {
            Display::None => return (0.0, 0.0),
            Display::Flex if style.flex_direction == FlexDirection::Row => {
                for child in children {
                    let child_style = self.style(child);
                    if child_style.position == Position::Absolute
                        || child_style.display == Display::None
                    {
                        continue;
                    }
                    let (cw, ch) = self.measure_node(child, inner_w, inner_h);
                    content_w += cw + child_style.margin.horizontal();
                    content_h = content_h.max(ch + child_style.margin.vertical());
                    if flow_count > 0 {
                        content_w += style.gap;
                    }
                    flow_count += 1;
                }
            }
            // Block and column flex both stack children along the vertical axis.
            Display::Block | Display::Flex => {
                for child in children {
                    let child_style = self.style(child);
                    if child_style.position == Position::Absolute
                        || child_style.display == Display::None
                    {
                        continue;
                    }
                    let (cw, ch) = self.measure_node(child, inner_w, inner_h);
                    content_w = content_w.max(cw + child_style.margin.horizontal());
                    content_h += ch + child_style.margin.vertical();
                    if flow_count > 0 {
                        content_h += style.gap;
                    }
                    flow_count += 1;
                }
            }
        }

        let width = explicit_w.unwrap_or(content_w + style.padding.horizontal());
        let height = explicit_h.unwrap_or(content_h + style.padding.vertical());
        (
            clamp_length(width, style.min_width, style.max_width, avail_w),
            clamp_length(height, style.min_height, style.max_height, avail_h),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn layout_node(
        &mut self,
        index: u32,
        x: f32,
        y: f32,
        avail_w: f32,
        avail_h: f32,
        forced_w: Option<f32>,
        forced_h: Option<f32>,
    ) -> (f32, f32) {
        let kind = self.document.document.nodes[index as usize].kind;
        let style = self.style(index);
        if style.display == Display::None {
            self.boxes[index as usize].rect = Rect::default();
            return (0.0, 0.0);
        }

        if kind == NodeKind::Text || kind == NodeKind::Image {
            let (measured_w, measured_h) = self.measure_node(index, avail_w, avail_h);
            let width =
                forced_w.unwrap_or_else(|| style.width.resolve(avail_w).unwrap_or(measured_w));
            let height =
                forced_h.unwrap_or_else(|| style.height.resolve(avail_h).unwrap_or(measured_h));
            let width = clamp_length(width, style.min_width, style.max_width, avail_w);
            let height = clamp_length(height, style.min_height, style.max_height, avail_h);
            self.boxes[index as usize].rect = Rect {
                x,
                y,
                width,
                height,
            };
            return (width, height);
        }

        let explicit_width = forced_w.or_else(|| style.width.resolve(avail_w));
        let mut height = forced_h.or_else(|| style.height.resolve(avail_h));
        let measured = if explicit_width.is_some() && height.is_some() {
            (0.0, 0.0)
        } else {
            self.measure_node(index, avail_w, avail_h)
        };
        let mut width = explicit_width.unwrap_or(measured.0.min(avail_w));
        width = clamp_length(width, style.min_width, style.max_width, avail_w);
        let provisional_height = height.unwrap_or(measured.1.min(avail_h));
        let content_x = x + style.padding.left;
        let content_y = y + style.padding.top;
        let content_w = (width - style.padding.horizontal()).max(0.0);
        let content_h = (provisional_height - style.padding.vertical()).max(0.0);

        self.boxes[index as usize].rect = Rect {
            x,
            y,
            width,
            height: provisional_height,
        };
        let children = self.children(index);
        let consumed = match style.display {
            Display::None => (0.0, 0.0),
            Display::Block => self.layout_block(
                children, content_x, content_y, content_w, content_h, style.gap,
            ),
            Display::Flex => {
                self.layout_flex(children, content_x, content_y, content_w, content_h, &style)
            }
        };

        if height.is_none() {
            height = Some(consumed.1 + style.padding.vertical());
        }
        let final_height = clamp_length(
            height.unwrap_or(0.0),
            style.min_height,
            style.max_height,
            avail_h,
        );
        self.boxes[index as usize].rect.height = final_height;

        self.layout_absolute_children(
            index,
            content_x,
            content_y,
            content_w,
            (final_height - style.padding.vertical()).max(0.0),
        );
        (width, final_height)
    }

    pub(super) fn layout_block(
        &mut self,
        children: impl Iterator<Item = u32>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        gap: f32,
    ) -> (f32, f32) {
        let mut cursor_y = y;
        let mut max_w: f32 = 0.0;
        let mut count = 0usize;
        for child in children {
            let child_style = self.style(child);
            if child_style.position == Position::Absolute || child_style.display == Display::None {
                continue;
            }
            if count > 0 {
                cursor_y += gap;
            }
            cursor_y += child_style.margin.top;
            let child_x = x + child_style.margin.left;
            let child_w = (width - child_style.margin.horizontal()).max(0.0);
            let forced_w = child_style.width.resolve(width).or(Some(child_w));
            let forced_h = child_style.height.resolve(height);
            let (actual_w, actual_h) = self.layout_node(
                child, child_x, cursor_y, child_w, height, forced_w, forced_h,
            );
            cursor_y += actual_h + child_style.margin.bottom;
            max_w = max_w.max(actual_w + child_style.margin.horizontal());
            count += 1;
        }
        (max_w, (cursor_y - y).max(0.0))
    }

    pub(super) fn layout_flex(
        &mut self,
        children: impl Iterator<Item = u32>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        style: &Style,
    ) -> (f32, f32) {
        // Collected: `flow.len()` and two ordered passes need the set up front.
        let flow: Vec<u32> = children
            .filter(|child| {
                let child_style = self.style(*child);
                child_style.position == Position::Flow && child_style.display != Display::None
            })
            .collect();
        if flow.is_empty() {
            return (0.0, 0.0);
        }

        let is_row = style.flex_direction == FlexDirection::Row;
        let main_available = if is_row { width } else { height };
        let cross_available = if is_row { height } else { width };
        let gap_total = style.gap * flow.len().saturating_sub(1) as f32;
        let mut bases = Vec::with_capacity(flow.len());
        for &child in &flow {
            let child_style = self.style(child);
            let measured = self.measure_node(child, width, height);
            let main = if is_row {
                child_style.width.resolve(width).unwrap_or(measured.0)
            } else {
                child_style.height.resolve(height).unwrap_or(measured.1)
            };
            bases.push((child, main, measured));
        }

        // Flex shrink (CSS default 1): when the items overflow the line, scroll containers give up
        // size down to their automatic minimum of 0 (CSS Flexbox 4.5), in proportion to their size.
        // Other items keep their content size as the minimum.
        let margin_main_of = |child: u32| {
            let child_style = self.style(child);
            if is_row {
                child_style.margin.horizontal()
            } else {
                child_style.margin.vertical()
            }
        };
        let scrolls_on_main = |child: u32| {
            let child_style = self.style(child);
            let overflow = if is_row {
                child_style.overflow_x
            } else {
                child_style.overflow_y
            };
            overflow != Overflow::Visible
        };
        let margins_total: f32 = flow.iter().map(|&child| margin_main_of(child)).sum();
        let content_total: f32 =
            bases.iter().map(|(_, main, _)| main).sum::<f32>() + margins_total + gap_total;
        let deficit = content_total - main_available;
        if deficit > 0.0 {
            let shrinkable: f32 = bases
                .iter()
                .filter(|(child, _, _)| scrolls_on_main(*child))
                .map(|(_, main, _)| *main)
                .sum();
            if shrinkable > 0.0 {
                let take = deficit.min(shrinkable);
                for (child, main, _) in bases.iter_mut() {
                    if scrolls_on_main(*child) {
                        *main -= take * *main / shrinkable;
                    }
                }
            }
        }

        let mut total_base = gap_total;
        let mut total_grow = 0.0f32;
        for (child, main, _) in &bases {
            total_base += main + margin_main_of(*child);
            total_grow += self.style(*child).flex_grow.max(0.0);
        }

        let remaining = (main_available - total_base).max(0.0);
        let used_main = total_base + if total_grow > 0.0 { remaining } else { 0.0 };
        let slack = (main_available - used_main).max(0.0);
        let (mut cursor, distributed_gap) = match style.justify_content {
            JustifyContent::Start => (0.0, style.gap),
            JustifyContent::Center => (slack / 2.0, style.gap),
            JustifyContent::End => (slack, style.gap),
            JustifyContent::SpaceBetween if flow.len() > 1 => {
                (0.0, style.gap + slack / (flow.len() - 1) as f32)
            }
            JustifyContent::SpaceBetween => (slack / 2.0, style.gap),
        };

        let mut max_cross: f32 = 0.0;
        for (slot, (child, base_main, measured)) in bases.into_iter().enumerate() {
            let child_style = self.style(child);
            if slot > 0 {
                cursor += distributed_gap;
            }
            let grow = if total_grow > 0.0 {
                remaining * (child_style.flex_grow.max(0.0) / total_grow)
            } else {
                0.0
            };
            let main = base_main + grow;
            let margin_main_before = if is_row {
                child_style.margin.left
            } else {
                child_style.margin.top
            };
            let margin_main_after = if is_row {
                child_style.margin.right
            } else {
                child_style.margin.bottom
            };
            let margin_cross_before = if is_row {
                child_style.margin.top
            } else {
                child_style.margin.left
            };
            let margin_cross_after = if is_row {
                child_style.margin.bottom
            } else {
                child_style.margin.right
            };
            cursor += margin_main_before;

            let explicit_cross = if is_row {
                child_style.height.resolve(height)
            } else {
                child_style.width.resolve(width)
            };
            let measured_cross = if is_row { measured.1 } else { measured.0 };
            let available_cross =
                (cross_available - margin_cross_before - margin_cross_after).max(0.0);
            let cross = match (explicit_cross, style.align_items) {
                (Some(value), _) => value.min(available_cross),
                (None, AlignItems::Stretch) => available_cross,
                (None, _) => measured_cross.min(available_cross),
            };
            let cross_offset = match style.align_items {
                AlignItems::Start | AlignItems::Stretch => margin_cross_before,
                AlignItems::Center => {
                    margin_cross_before + (available_cross - cross).max(0.0) / 2.0
                }
                AlignItems::End => margin_cross_before + (available_cross - cross).max(0.0),
            };

            let (child_x, child_y, forced_w, forced_h) = if is_row {
                (x + cursor, y + cross_offset, Some(main), Some(cross))
            } else {
                (x + cross_offset, y + cursor, Some(cross), Some(main))
            };
            let (actual_w, actual_h) =
                self.layout_node(child, child_x, child_y, width, height, forced_w, forced_h);
            let actual_cross = if is_row { actual_h } else { actual_w };
            max_cross = max_cross.max(actual_cross + margin_cross_before + margin_cross_after);
            cursor += main + margin_main_after;
        }

        if is_row {
            (cursor, max_cross)
        } else {
            (max_cross, cursor)
        }
    }

    pub(super) fn layout_absolute_children(
        &mut self,
        parent: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        for child in self.children(parent) {
            self.place_absolute_child(child, x, y, width, height);
        }
    }

    /// Places one absolutely positioned child inside its containing block. Shared by the full
    /// layout pass and the isolated geometry patch so both produce identical boxes.
    pub(super) fn place_absolute_child(
        &mut self,
        child: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let style = self.style(child);
        if style.position != Position::Absolute || style.display == Display::None {
            return;
        }
        {
            let left = style.left.resolve_offset(width);
            let right = style.right.resolve_offset(width);
            let top = style.top.resolve_offset(height);
            let bottom = style.bottom.resolve_offset(height);
            let forced_w = style.width.resolve(width).or_else(|| match (left, right) {
                (Some(left), Some(right)) => Some((width - left - right).max(0.0)),
                _ => None,
            });
            let forced_h = style
                .height
                .resolve(height)
                .or_else(|| match (top, bottom) {
                    (Some(top), Some(bottom)) => Some((height - top - bottom).max(0.0)),
                    _ => None,
                });
            let measured = self.measure_node(child, width, height);
            let child_w = forced_w.unwrap_or(measured.0);
            let child_h = forced_h.unwrap_or(measured.1);
            let child_x = x + left
                .unwrap_or_else(|| right.map(|value| width - value - child_w).unwrap_or(0.0));
            let child_y = y + top
                .unwrap_or_else(|| bottom.map(|value| height - value - child_h).unwrap_or(0.0));
            self.layout_node(
                child,
                child_x,
                child_y,
                width,
                height,
                Some(child_w),
                Some(child_h),
            );
        }
    }
}
