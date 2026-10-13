use crate::model::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct LayoutResult {
    pub boxes: Vec<LayoutBox>,
    /// Content extent per node in surface coordinates (viewport origin +
    /// intrinsic content size). Viewport is `boxes[index].rect`.
    pub contents: Vec<Rect>,
    /// Document revision the retained boxes were computed from.
    pub revision: u64,
    /// Cached z-order (ascending) for paint + hit-test walks. No re-sort.
    pub z_order: Vec<u32>,
}

impl LayoutResult {
    pub fn content_extent(&self, index: u32) -> Rect {
        self.contents
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Pure viewport/content/offset -> metrics (thumb geometry included).
    /// Thumb minimum is 12px; no thumb when content fits.
    pub fn scroll_metrics(viewport: Rect, content: Rect, offset: ScrollState) -> ScrollMetrics {
        let max_x = (content.width - viewport.width).max(0.0);
        let max_y = (content.height - viewport.height).max(0.0);
        let offset = offset.clamped(max_x, max_y);
        ScrollMetrics {
            viewport,
            content,
            max_x,
            max_y,
            thumb_x: thumb_rect(
                viewport,
                content.width,
                viewport.width,
                offset.offset_x,
                true,
            ),
            thumb_y: thumb_rect(
                viewport,
                content.height,
                viewport.height,
                offset.offset_y,
                false,
            ),
        }
    }

    pub fn metrics_for(&self, index: u32, offset: ScrollState) -> ScrollMetrics {
        let viewport = self
            .boxes
            .get(index as usize)
            .map(|b| b.rect)
            .unwrap_or_default();
        Self::scroll_metrics(viewport, self.content_extent(index), offset)
    }

    pub fn hit_test_action(&self, document: &RuntimeDocument, x: f32, y: f32) -> Option<u32> {
        for index in self.z_order.iter().copied().rev() {
            let index = index as usize;
            let node = &document.document.nodes[index];
            if node.action.is_empty() || !document.is_effectively_visible(index as u32) {
                continue;
            }
            if self.boxes.get(index)?.rect.contains(x, y) {
                return Some(index as u32);
            }
        }
        None
    }
}

pub struct LayoutEngine;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntrinsicSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntrinsicMeasureError {
    NonFinite,
    NonPositive,
    ExceedsConstraint,
}

impl std::fmt::Display for IntrinsicMeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => write!(f, "intrinsic constraint must be finite"),
            Self::NonPositive => write!(f, "intrinsic constraint must be positive"),
            Self::ExceedsConstraint => write!(f, "intrinsic size exceeds constraint"),
        }
    }
}

impl std::error::Error for IntrinsicMeasureError {}

impl LayoutEngine {
    /// Outer intrinsic size of one document node under finite max constraints.
    pub fn measure_node(
        document: &RuntimeDocument,
        node: u32,
        max_w: f32,
        max_h: f32,
        interaction: InteractionState,
    ) -> Result<IntrinsicSize, IntrinsicMeasureError> {
        if !max_w.is_finite() || !max_h.is_finite() {
            return Err(IntrinsicMeasureError::NonFinite);
        }
        if max_w <= 0.0 || max_h <= 0.0 {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        if (node as usize) >= document.document.nodes.len() {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        let mut empty: Vec<LayoutBox> = Vec::new();
        let probe = Engine {
            document,
            interaction,
            boxes: &mut empty,
        };
        // Finite popup constraint only; never f32::MAX here.
        let (w, h) = probe.measure_node(node, max_w, max_h);
        if !w.is_finite() || !h.is_finite() {
            return Err(IntrinsicMeasureError::NonFinite);
        }
        if w <= 0.0 || h <= 0.0 {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        if w > max_w || h > max_h {
            return Err(IntrinsicMeasureError::ExceedsConstraint);
        }
        Ok(IntrinsicSize {
            width: w,
            height: h,
        })
    }

    /// Outer intrinsic size of the document root under finite max constraints.
    /// Uses `measure_node(root, max)` only; never reads
    /// `layout.contents[root]` / `content_extent(root)` (scroll metrics owner).
    pub fn measure_root(
        document: &RuntimeDocument,
        max_w: f32,
        max_h: f32,
        interaction: InteractionState,
    ) -> Result<IntrinsicSize, IntrinsicMeasureError> {
        Self::measure_node(document, document.document.root, max_w, max_h, interaction)
    }
    pub fn compute(
        document: &RuntimeDocument,
        viewport_width: f32,
        viewport_height: f32,
        interaction: InteractionState,
    ) -> LayoutResult {
        let mut boxes = vec![LayoutBox::default(); document.document.nodes.len()];
        if document.document.nodes.is_empty() {
            return LayoutResult {
                boxes,
                contents: Vec::new(),
                revision: document.revision(),
                z_order: Vec::new(),
            };
        }
        let mut engine = Engine {
            document,
            interaction,
            boxes: &mut boxes,
        };
        engine.layout_node(
            document.document.root,
            0.0,
            0.0,
            viewport_width.max(0.0),
            viewport_height.max(0.0),
            Some(viewport_width.max(0.0)),
            Some(viewport_height.max(0.0)),
        );
        // Content extent: intrinsic measure unconstrained on the scroll axis.
        let mut contents = vec![Rect::default(); document.document.nodes.len()];
        for (index, entry) in boxes.iter().enumerate() {
            let style = document.runtime_style(index as u32, interaction);
            let scrolls_x = matches!(
                style.overflow_x,
                Overflow::Auto | Overflow::Scroll | Overflow::Hidden
            );
            let scrolls_y = matches!(
                style.overflow_y,
                Overflow::Auto | Overflow::Scroll | Overflow::Hidden
            );
            if !scrolls_x && !scrolls_y {
                contents[index] = entry.rect;
                continue;
            }
            let mut empty: Vec<LayoutBox> = Vec::new();
            let probe = Engine {
                document,
                interaction,
                boxes: &mut empty,
            };
            let avail_w = if scrolls_x {
                f32::MAX / 1024.0
            } else {
                entry.rect.width.max(0.0)
            };
            let avail_h = if scrolls_y {
                f32::MAX / 1024.0
            } else {
                entry.rect.height.max(0.0)
            };
            let measured = probe.measure_node(index as u32, avail_w, avail_h);
            let content_w = if scrolls_x {
                measured.0.max(entry.rect.width)
            } else {
                entry.rect.width
            };
            let content_h = if scrolls_y {
                measured.1.max(entry.rect.height)
            } else {
                entry.rect.height
            };
            contents[index] = Rect {
                x: entry.rect.x,
                y: entry.rect.y,
                width: content_w,
                height: content_h,
            };
        }
        let mut z_order: Vec<u32> = (0..document.document.nodes.len() as u32).collect();
        z_order.sort_by_key(|index| (document.effective_z_index(*index), *index as i64));
        LayoutResult {
            boxes,
            contents,
            revision: document.revision(),
            z_order,
        }
    }
}

struct Engine<'a> {
    document: &'a RuntimeDocument,
    interaction: InteractionState,
    boxes: &'a mut [LayoutBox],
}

impl<'a> Engine<'a> {
    fn style(&self, index: u32) -> Style {
        self.document.runtime_style(index, self.interaction)
    }

    /// Sibling-link walk in document order. Borrows the document (`'a`), not
    /// `self`, so callers may mutate boxes while iterating.
    fn children(&self, index: u32) -> impl Iterator<Item = u32> + 'a {
        let document: &'a RuntimeDocument = self.document;
        let nodes = &document.document.nodes;
        let mut cursor = nodes[index as usize].first_child;
        std::iter::from_fn(move || {
            let child = cursor?;
            cursor = nodes[child as usize].next_sibling;
            Some(child)
        })
    }

    fn measure_node(&self, index: u32, avail_w: f32, avail_h: f32) -> (f32, f32) {
        let node = &self.document.document.nodes[index as usize];
        let style = self.style(index);
        if style.display == Display::None {
            return (0.0, 0.0);
        }
        if node.kind == NodeKind::Text {
            let text = self.document.text_for(index);
            let laid = crate::text_layout::layout_text(
                text,
                style.font_size,
                avail_w,
                style.text_wrap,
                style.break_anywhere,
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
        let inner_w = explicit_w
            .unwrap_or(avail_w)
            .max(style.padding.horizontal())
            - style.padding.horizontal();
        let inner_h =
            explicit_h.unwrap_or(avail_h).max(style.padding.vertical()) - style.padding.vertical();
        let children = self.children(index);
        let mut content_w: f32 = 0.0;
        let mut content_h: f32 = 0.0;
        let mut flow_count = 0usize;

        match style.display {
            Display::None => return (0.0, 0.0),
            Display::Block => {
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
            Display::Flex => match style.flex_direction {
                FlexDirection::Row => {
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
                FlexDirection::Column => {
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
            },
        }

        let width = explicit_w.unwrap_or(content_w + style.padding.horizontal());
        let height = explicit_h.unwrap_or(content_h + style.padding.vertical());
        (
            clamp_length(width, style.min_width, style.max_width, avail_w),
            clamp_length(height, style.min_height, style.max_height, avail_h),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_node(
        &mut self,
        index: u32,
        x: f32,
        y: f32,
        avail_w: f32,
        avail_h: f32,
        forced_w: Option<f32>,
        forced_h: Option<f32>,
    ) -> (f32, f32) {
        let node = self.document.document.nodes[index as usize].clone();
        let style = self.style(index);
        if style.display == Display::None {
            self.boxes[index as usize].rect = Rect::default();
            return (0.0, 0.0);
        }

        if node.kind == NodeKind::Text || node.kind == NodeKind::Image {
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

        let measured = self.measure_node(index, avail_w, avail_h);
        let mut width = forced_w
            .or_else(|| style.width.resolve(avail_w))
            .unwrap_or(measured.0.min(avail_w));
        let mut height = forced_h.or_else(|| style.height.resolve(avail_h));
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

    fn layout_block(
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

    fn layout_flex(
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
        let mut total_base = gap_total;
        let mut total_grow = 0.0f32;

        for &child in &flow {
            let child_style = self.style(child);
            let measured = self.measure_node(child, width, height);
            let main = if is_row {
                child_style.width.resolve(width).unwrap_or(measured.0)
            } else {
                child_style.height.resolve(height).unwrap_or(measured.1)
            };
            let margin_main = if is_row {
                child_style.margin.horizontal()
            } else {
                child_style.margin.vertical()
            };
            total_base += main + margin_main;
            total_grow += child_style.flex_grow.max(0.0);
            bases.push((child, main, measured));
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

    fn layout_absolute_children(&mut self, parent: u32, x: f32, y: f32, width: f32, height: f32) {
        for child in self.children(parent) {
            let style = self.style(child);
            if style.position != Position::Absolute || style.display == Display::None {
                continue;
            }
            let left = style.left.resolve(width);
            let right = style.right.resolve(width);
            let top = style.top.resolve(height);
            let bottom = style.bottom.resolve(height);
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

fn clamp_length(value: f32, min: Length, max: Length, available: f32) -> f32 {
    let mut result = value.max(0.0);
    if let Some(minimum) = min.resolve(available) {
        result = result.max(minimum);
    }
    if let Some(maximum) = max.resolve(available) {
        result = result.min(maximum);
    }
    result
}

fn thumb_rect(
    viewport: Rect,
    content_len: f32,
    viewport_len: f32,
    offset: f32,
    horizontal: bool,
) -> Rect {
    // C01: never panic on nonfinite/zero/negative/tiny geometry.
    if !viewport_len.is_finite()
        || !content_len.is_finite()
        || viewport_len <= 0.0
        || content_len <= 0.0
        || content_len <= viewport_len
    {
        return Rect::default();
    }
    if !viewport.x.is_finite()
        || !viewport.y.is_finite()
        || !viewport.width.is_finite()
        || !viewport.height.is_finite()
    {
        return Rect::default();
    }
    let track = if horizontal {
        viewport.width
    } else {
        viewport.height
    };
    if !track.is_finite() || track <= 0.0 {
        return Rect::default();
    }
    let raw = viewport_len / content_len * track;
    if !raw.is_finite() {
        return Rect::default();
    }
    let min_thumb = 12.0f32.min(track);
    let thumb_len = raw.clamp(min_thumb, track);
    if !thumb_len.is_finite() {
        return Rect::default();
    }
    let travel = (track - thumb_len).max(0.0);
    let max_offset = (content_len - viewport_len).max(1.0);
    if !max_offset.is_finite() || max_offset <= 0.0 {
        return Rect::default();
    }
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let pos = (offset.clamp(0.0, max_offset) / max_offset) * travel;
    if !pos.is_finite() {
        return Rect::default();
    }
    if horizontal {
        Rect {
            x: viewport.x + pos,
            y: viewport.y + viewport.height - 8.0,
            width: thumb_len,
            height: 8.0,
        }
    } else {
        Rect {
            x: viewport.x + viewport.width - 8.0,
            y: viewport.y + pos,
            width: 8.0,
            height: thumb_len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_node(parent: u32, next: Option<u32>, text: &str) -> CompiledNode {
        CompiledNode {
            kind: NodeKind::Text,
            parent: Some(parent),
            first_child: None,
            next_sibling: next,
            id: String::new(),
            action: String::new(),
            text: text.into(),
            image: None,
            style: Style::default(),
            hover_style: None,
            active_style: None,
        }
    }

    #[test]
    fn flex_grow_fills_available_width() {
        let mut left = Style::default();
        left.flex_grow = 1.0;
        let mut right = Style::default();
        right.width = Length::Px(20.0);
        let mut root_style = Style::default();
        root_style.display = Display::Flex;
        root_style.height = Length::Px(40.0);
        let document = CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: None,
                    first_child: Some(1),
                    next_sibling: None,
                    id: String::new(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: root_style,
                    hover_style: None,
                    active_style: None,
                },
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: Some(0),
                    first_child: Some(3),
                    next_sibling: Some(2),
                    id: String::new(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: left,
                    hover_style: None,
                    active_style: None,
                },
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: Some(0),
                    first_child: None,
                    next_sibling: None,
                    id: String::new(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: right,
                    hover_style: None,
                    active_style: None,
                },
                text_node(1, None, "left"),
            ],
        };
        let runtime = RuntimeDocument::new(document).unwrap();
        let layout = LayoutEngine::compute(&runtime, 100.0, 40.0, InteractionState::default());
        assert!((layout.boxes[1].rect.width - 80.0).abs() < 0.01);
    }

    fn metrics(viewport_w: f32, content_w: f32) -> ScrollMetrics {
        LayoutResult::scroll_metrics(
            Rect {
                x: 0.0,
                y: 0.0,
                width: viewport_w,
                height: 20.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: content_w,
                height: 40.0,
            },
            ScrollState::default(),
        )
    }

    #[test]
    fn thumb_tiny_tracks_never_panic() {
        for track in [1.0f32, 11.0, 12.0, 100.0] {
            let m = metrics(track, track * 4.0);
            assert!(m.thumb_x.width.is_finite());
            assert!(m.thumb_x.width <= track + 0.001);
            if track < 12.0 {
                assert!((m.thumb_x.width - track).abs() < 0.01);
            }
        }
        // Nonfinite / zero / non-scrollable all yield default thumb.
        for (vw, cw) in [
            (f32::NAN, 100.0),
            (100.0, f32::INFINITY),
            (0.0, 100.0),
            (100.0, 50.0),
            (100.0, 100.0),
            (-10.0, 100.0),
        ] {
            let m = metrics(vw, cw);
            assert_eq!(m.thumb_x, Rect::default());
        }
    }

    fn fixed_doc(w: f32, h: f32) -> RuntimeDocument {
        let mut root_style = Style::default();
        root_style.width = Length::Px(w);
        root_style.height = Length::Px(h);
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: String::new(),
                action: String::new(),
                text: String::new(),
                image: None,
                style: root_style,
                hover_style: None,
                active_style: None,
            }],
        })
        .unwrap()
    }

    fn overflow_auto_doc() -> RuntimeDocument {
        let mut root_style = Style::default();
        root_style.width = Length::Px(200.0);
        root_style.height = Length::Px(100.0);
        root_style.overflow_y = Overflow::Auto;
        let mut child_style = Style::default();
        child_style.width = Length::Px(180.0);
        child_style.height = Length::Px(500.0);
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: None,
                    first_child: Some(1),
                    next_sibling: None,
                    id: String::new(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: root_style,
                    hover_style: None,
                    active_style: None,
                },
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: Some(0),
                    first_child: None,
                    next_sibling: None,
                    id: String::new(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: child_style,
                    hover_style: None,
                    active_style: None,
                },
            ],
        })
        .unwrap()
    }

    fn popup_wrapper_doc() -> RuntimeDocument {
        let mut wrapper_style = Style::default();
        wrapper_style.width = Length::Percent(1.0);
        wrapper_style.height = Length::Percent(1.0);
        let mut popup_style = Style::default();
        popup_style.width = Length::Px(200.0);
        popup_style.height = Length::Px(100.0);
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: None,
                    first_child: Some(1),
                    next_sibling: None,
                    id: "wrapper".into(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: wrapper_style,
                    hover_style: None,
                    active_style: None,
                },
                CompiledNode {
                    kind: NodeKind::Element,
                    parent: Some(0),
                    first_child: None,
                    next_sibling: None,
                    id: "popup".into(),
                    action: String::new(),
                    text: String::new(),
                    image: None,
                    style: popup_style,
                    hover_style: None,
                    active_style: None,
                },
            ],
        })
        .unwrap()
    }

    #[test]
    fn measure_root_finite_fixed_popup() {
        let doc = fixed_doc(200.0, 100.0);
        let size = LayoutEngine::measure_root(&doc, 800.0, 600.0, InteractionState::default())
            .expect("finite fixed popup measures");
        assert_eq!((size.width, size.height), (200.0, 100.0));
    }

    #[test]
    fn measure_root_overflow_auto_child_not_max() {
        let doc = overflow_auto_doc();
        let size = LayoutEngine::measure_root(&doc, 800.0, 600.0, InteractionState::default())
            .expect("overflow:auto root measures");
        assert!(size.width.is_finite() && size.height.is_finite());
        assert!(size.width < f32::MAX / 2.0 && size.height < f32::MAX / 2.0);
        assert!(size.width <= 800.0 && size.height <= 600.0);
        assert_eq!((size.width, size.height), (200.0, 100.0));
    }

    #[test]
    fn measure_node_selects_compact_popup_over_constraint_sized_wrapper() {
        let doc = popup_wrapper_doc();
        let popup = doc.node_by_id("popup").expect("popup node exists");
        let interaction = InteractionState::default();
        let popup_size = LayoutEngine::measure_node(&doc, popup, 800.0, 600.0, interaction)
            .expect("fixed popup measures");
        let root_size = LayoutEngine::measure_root(&doc, 800.0, 600.0, interaction)
            .expect("wrapper root measures");
        assert_eq!((popup_size.width, popup_size.height), (200.0, 100.0));
        assert_eq!((root_size.width, root_size.height), (800.0, 600.0));
    }

    /// Nested flow/flex/absolute/display:none/scroll fixture. Child order is
    /// the sibling-link order, including absolute and display:none children.
    fn traversal_fixture() -> RuntimeDocument {
        let node = |parent: Option<u32>,
                    first_child: Option<u32>,
                    next_sibling: Option<u32>,
                    kind: NodeKind,
                    text: &str,
                    style: Style| CompiledNode {
            kind,
            parent,
            first_child,
            next_sibling,
            id: String::new(),
            action: String::new(),
            text: text.into(),
            image: None,
            style,
            hover_style: None,
            active_style: None,
        };
        let px = |w: f32, h: f32| {
            let mut s = Style::default();
            s.width = Length::Px(w);
            s.height = Length::Px(h);
            s
        };
        let mut root = px(300.0, 200.0);
        root.overflow_y = Overflow::Auto;
        let mut row = px(120.0, 40.0);
        row.display = Display::Flex;
        row.flex_direction = FlexDirection::Row;
        let mut abs = px(50.0, 30.0);
        abs.position = Position::Absolute;
        abs.right = Length::Px(0.0);
        abs.top = Length::Px(0.0);
        let mut hidden = px(10.0, 10.0);
        hidden.display = Display::None;
        let mut hidden_leaf = Style::default();
        hidden_leaf.display = Display::None;
        let mut col = Style::default();
        col.display = Display::Flex;
        col.flex_direction = FlexDirection::Column;
        col.height = Length::Px(60.0);
        let mut grow = Style::default();
        grow.flex_grow = 1.0;
        let mut inner_abs = px(15.0, 15.0);
        inner_abs.position = Position::Absolute;
        inner_abs.left = Length::Px(10.0);
        inner_abs.top = Length::Px(5.0);
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                // 0 root (scroll container): children 1, 2, 3, 4
                node(None, Some(1), None, NodeKind::Element, "", root),
                // 1 flex row: children 5, 6, 7
                node(Some(0), Some(5), Some(2), NodeKind::Element, "", row),
                // 2 absolute: no children, sibling 3
                node(Some(0), None, Some(3), NodeKind::Element, "", abs),
                // 3 display:none with a child
                node(Some(0), Some(8), Some(4), NodeKind::Element, "", hidden),
                // 4 flex column: children 9, 10
                node(Some(0), Some(9), None, NodeKind::Element, "", col),
                // 5 flow leaf
                node(
                    Some(1),
                    None,
                    Some(6),
                    NodeKind::Element,
                    "",
                    px(30.0, 20.0),
                ),
                // 6 absolute child of flex row
                node(Some(1), None, Some(7), NodeKind::Element, "", inner_abs),
                // 7 display:none in flex row
                node(Some(1), None, None, NodeKind::Element, "", hidden_leaf),
                // 8 child of display:none
                node(
                    Some(3),
                    None,
                    None,
                    NodeKind::Text,
                    "hidden",
                    Style::default(),
                ),
                // 9 grow child
                node(Some(4), None, Some(10), NodeKind::Element, "", grow),
                // 10 text
                node(Some(4), None, None, NodeKind::Text, "hi", Style::default()),
            ],
        })
        .unwrap()
    }

    #[test]
    fn traversal_order_and_boxes_match_fixture() {
        let doc = traversal_fixture();
        let engine = Engine {
            document: &doc,
            interaction: InteractionState::default(),
            boxes: &mut [],
        };
        assert_eq!(engine.children(0).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(engine.children(1).collect::<Vec<_>>(), vec![5, 6, 7]);
        assert_eq!(engine.children(3).collect::<Vec<_>>(), vec![8]);
        assert_eq!(engine.children(5).collect::<Vec<_>>(), Vec::<u32>::new());
        let layout = LayoutEngine::compute(&doc, 300.0, 200.0, InteractionState::default());
        let rects: Vec<Rect> = layout.boxes.iter().map(|b| b.rect).collect();
        // Recorded from the pre-optimisation Vec-returning traversal.
        let expected: [(f32, f32, f32, f32); 11] = [
            (0.0, 0.0, 300.0, 200.0),
            (0.0, 0.0, 120.0, 40.0),
            (250.0, 0.0, 50.0, 30.0),
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 40.0, 300.0, 60.0),
            (0.0, 0.0, 30.0, 20.0),
            (10.0, 5.0, 15.0, 15.0),
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 40.0, 300.0, 41.8),
            (0.0, 81.8, 300.0, 18.2),
        ];
        assert_eq!(rects.len(), expected.len());
        for (index, (rect, (x, y, w, h))) in rects.iter().zip(expected).enumerate() {
            assert!(
                (rect.x - x).abs() < 0.01
                    && (rect.y - y).abs() < 0.01
                    && (rect.width - w).abs() < 0.01
                    && (rect.height - h).abs() < 0.01,
                "node {index}: got {rect:?}, expected ({x},{y},{w},{h})"
            );
        }
    }

    #[test]
    fn measure_root_refuses_bad_constraints() {
        let doc = fixed_doc(200.0, 100.0);
        for (w, h) in [
            (f32::NAN, 100.0),
            (100.0, f32::INFINITY),
            (f32::INFINITY, f32::INFINITY),
            (0.0, 100.0),
            (100.0, 0.0),
            (-10.0, 100.0),
            (100.0, -5.0),
        ] {
            let err = LayoutEngine::measure_root(&doc, w, h, InteractionState::default())
                .expect_err("bad constraint must fail");
            assert!(
                err == IntrinsicMeasureError::NonFinite
                    || err == IntrinsicMeasureError::NonPositive,
                "unexpected {err:?} for ({w},{h})"
            );
        }
    }

    #[test]
    fn measure_root_result_over_constraint_refused() {
        let doc = fixed_doc(500.0, 400.0);
        let err = LayoutEngine::measure_root(&doc, 100.0, 100.0, InteractionState::default())
            .expect_err("oversize result must fail");
        assert_eq!(err, IntrinsicMeasureError::ExceedsConstraint);
    }
}
