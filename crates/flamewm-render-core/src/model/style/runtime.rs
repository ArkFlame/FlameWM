use super::*;

impl RuntimeDocument {
    pub fn set_color_variable(&mut self, name: &str, color: Color) -> Result<(), String> {
        let index = self
            .variable_by_name(name)
            .ok_or_else(|| format!("no color variable named '{name}'"))?;
        if self.variable_values[index as usize] != color {
            self.variable_values[index as usize] = color;
            self.touch_paint();
        }
        Ok(())
    }
}

/// Stores `value` in `slot` and reports whether the stored value actually changed.
fn set_slot<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

impl RuntimeDocument {
    fn override_entry(&mut self, id: &str) -> Result<&mut RuntimeStyleOverride, String> {
        let index = self.require_node(id)?;
        Ok(self.style_overrides.entry(index).or_default())
    }

    fn set_literal_color(
        &mut self,
        id: &str,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<ColorValue>,
        color: Color,
    ) -> Result<(), String> {
        let index = self.require_node(id)?;
        let value = Some(ColorValue::Literal(color));
        if set_slot(field(self.style_overrides.entry(index).or_default()), value) {
            self.touch_node(index, DirtyReason::Paint, false);
        }
        Ok(())
    }

    fn clear_color_field(
        &mut self,
        id: &str,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<ColorValue>,
    ) -> Result<(), String> {
        let index = self.require_node(id)?;
        let changed = self
            .style_overrides
            .get_mut(&index)
            .is_some_and(|style| field(style).take().is_some());
        if changed {
            self.touch_node(index, DirtyReason::Paint, false);
        }
        Ok(())
    }

    pub fn set_background_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        self.set_literal_color(id, |style| &mut style.background, color)
    }

    /// Per-node backdrop request: the root pixels beneath a translucent node are captured,
    /// blurred by `blur_percent` and laid under its fill. `None` or `Some(0)` clears it.
    pub fn set_node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        if blur_percent.is_some_and(|blur| blur > 100) {
            return Err("node backdrop blur must be 0..=100".to_string());
        }
        let index = self.require_node(id)?;
        let changed = match blur_percent.filter(|blur| *blur > 0) {
            Some(blur) => self.node_backdrops.insert(index, blur) != Some(blur),
            None => self.node_backdrops.remove(&index).is_some(),
        };
        if changed {
            self.touch();
        }
        Ok(())
    }

    /// Blur percent requested for a node's backdrop, if any.
    pub fn node_backdrop_blur(&self, index: u32) -> Option<u8> {
        self.node_backdrops.get(&index).copied()
    }

    /// Runtime corner radius in px for a node (popup/menu shape follows the decoration setting).
    pub fn set_border_radius(&mut self, id: &str, radius: f32) -> Result<(), String> {
        if !radius.is_finite() || radius < 0.0 {
            return Err("runtime border radius must be finite and non-negative".to_string());
        }
        // Panel geometry may be projected on every settings/reconcile tick.
        // Unchanged shape values must not invalidate the retained scene.
        let entry = self.override_entry(id)?;
        if entry.border_radius != Some(radius) {
            entry.border_radius = Some(radius);
            self.touch();
        }
        Ok(())
    }

    pub fn set_border_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        self.set_literal_color(id, |style| &mut style.border_color, color)
    }

    pub fn clear_background_color(&mut self, id: &str) -> Result<(), String> {
        self.clear_color_field(id, |style| &mut style.background)
    }

    pub fn clear_border_color(&mut self, id: &str) -> Result<(), String> {
        self.clear_color_field(id, |style| &mut style.border_color)
    }

    pub fn set_foreground_color(&mut self, id: &str, color: ColorValue) -> Result<(), String> {
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.color, color) {
            self.touch_paint();
        }
        Ok(())
    }

    /// Foreground taken from the named colour custom property, so it follows `set_color_variable`.
    pub fn set_foreground_variable(&mut self, id: &str, name: &str) -> Result<(), String> {
        let index = self
            .variable_by_name(name)
            .ok_or_else(|| format!("no color variable named '{name}'"))?;
        self.set_foreground_color(id, ColorValue::Variable(index))
    }

    pub fn clear_foreground_color(&mut self, id: &str) -> Result<(), String> {
        let index = self.require_node(id)?;
        let changed = self
            .style_overrides
            .get_mut(&index)
            .is_some_and(|style| style.color.take().is_some());
        if changed {
            self.touch_paint();
        }
        Ok(())
    }

    pub fn set_overflow(&mut self, id: &str, x: Overflow, y: Overflow) -> Result<(), String> {
        let entry = self.override_entry(id)?;
        let changed =
            set_slot(&mut entry.overflow_x, Some(x)) | set_slot(&mut entry.overflow_y, Some(y));
        if changed {
            self.touch();
        }
        Ok(())
    }

    pub fn set_font_size_px(&mut self, id: &str, size: f32) -> Result<(), String> {
        if !size.is_finite() || size <= 0.0 {
            return Err("runtime font size must be finite and positive".to_string());
        }
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.font_size, size) {
            self.touch();
        }
        Ok(())
    }

    /// Text children are measured with their own compile-time style, so an element override alone
    /// never reaches the glyphs: apply the same override to the element's direct text children.
    fn mirror_to_text_children(
        &mut self,
        index: u32,
        apply: impl Fn(&mut RuntimeStyleOverride) -> bool,
    ) -> bool {
        let mut changed = false;
        let mut child = self.document.nodes[index as usize].first_child;
        while let Some(current) = child {
            let node = &self.document.nodes[current as usize];
            let next = node.next_sibling;
            if node.kind == NodeKind::Text {
                changed |= apply(self.style_overrides.entry(current).or_default());
            }
            child = next;
        }
        changed
    }

    /// Sets one override field on `index` and the same value on its direct text children.
    /// Returns whether any stored value changed.
    fn set_override_and_mirror<T: PartialEq + Copy>(
        &mut self,
        index: u32,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<T>,
        value: T,
    ) -> bool {
        let mut changed = set_slot(
            field(self.style_overrides.entry(index).or_default()),
            Some(value),
        );
        changed |= self.mirror_to_text_children(index, |style| set_slot(field(style), Some(value)));
        changed
    }

    /// Multiplies the paint alpha of `id` and all its descendants by `opacity` (clamped to 0..=1).
    /// Layout and hit testing are unchanged.
    pub fn set_group_opacity(&mut self, id: &str, opacity: f32) -> Result<(), String> {
        let index = self.require_node(id)?;
        let value = opacity.clamp(0.0, 1.0);
        let changed = if value >= 1.0 {
            self.group_opacity.remove(&index).is_some()
        } else {
            self.group_opacity.insert(index, value) != Some(value)
        };
        if changed {
            self.touch();
        }
        Ok(())
    }

    /// Product of the group opacities of `index` and its ancestors. Cheap when none are set.
    fn group_opacity_factor(&self, index: u32) -> f32 {
        if self.group_opacity.is_empty() {
            return 1.0;
        }
        let mut factor = 1.0;
        let mut node = Some(index);
        while let Some(current) = node {
            factor *= self.group_opacity.get(&current).copied().unwrap_or(1.0);
            node = self.document.nodes[current as usize].parent;
        }
        factor
    }

    pub fn set_font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
        let weight = weight.clamp(100, 900);
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.font_weight, weight) {
            // The weight only re-measures this element's own text when it is an isolated box.
            if self.touch_isolated_box(index, DirtyReason::Text) {
                self.dirty.full = true;
            } else {
                self.touch();
            }
        }
        Ok(())
    }

    pub fn set_ui_font_family(&mut self, family: impl Into<String>) -> Result<(), String> {
        let family = family.into();
        if family.trim().is_empty() {
            return Err("UI font family cannot be empty".to_string());
        }
        self.ui_font_family = family;
        Ok(())
    }

    pub fn ui_font_family(&self) -> &str {
        &self.ui_font_family
    }

    /// Applies the global text size offset and minimum weight to every computed style.
    pub fn set_ui_font_defaults(&mut self, size_offset: f32, bold: bool) -> Result<(), String> {
        if !size_offset.is_finite() {
            return Err("UI font size offset must be finite".to_string());
        }
        if self.ui_font_size_offset != size_offset || self.ui_font_bold != bold {
            self.ui_font_size_offset = size_offset;
            self.ui_font_bold = bold;
            self.touch();
        }
        Ok(())
    }

    /// Global on/off for the shared text shadow token; off paints every text run without it.
    pub fn set_ui_text_shadow(&mut self, enabled: bool) {
        if self.ui_text_shadow != enabled {
            self.ui_text_shadow = enabled;
            self.touch_paint();
        }
    }

    pub fn ui_text_shadow(&self) -> bool {
        self.ui_text_shadow
    }

    pub fn set_ui_scale(&mut self, scale: f32) -> Result<(), String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("UI scale must be finite and positive".to_string());
        }
        let scale = scale.clamp(0.5, 4.0);
        if self.ui_scale != scale {
            self.ui_scale = scale;
            self.touch();
        }
        Ok(())
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    pub fn set_z_index(&mut self, id: &str, z: i32) -> Result<(), String> {
        let index = self.require_node(id)?;
        self.z_index_overrides.insert(index, z);
        Ok(())
    }

    pub fn effective_z_index(&self, mut index: u32) -> i32 {
        if self.z_index_overrides.is_empty() {
            return 0;
        }
        loop {
            if let Some(z) = self.z_index_overrides.get(&index) {
                return *z;
            }
            let Some(parent) = self.document.nodes[index as usize].parent else {
                return 0;
            };
            index = parent;
        }
    }

    /// `effective_z_index` of every node, resolved once per ancestor chain. `None` when no node
    /// carries a z-index override, so every node sits at 0 and document order is paint order.
    pub fn effective_z_indices(&self) -> Option<Vec<i32>> {
        if self.z_index_overrides.is_empty() {
            return None;
        }
        let nodes = &self.document.nodes;
        let mut resolved: Vec<Option<i32>> = vec![None; nodes.len()];
        let mut chain = Vec::new();
        for start in 0..nodes.len() as u32 {
            let mut index = start;
            let z = loop {
                if let Some(z) = resolved[index as usize] {
                    break z;
                }
                if let Some(&z) = self.z_index_overrides.get(&index) {
                    break z;
                }
                chain.push(index);
                match nodes[index as usize].parent {
                    Some(parent) if chain.len() <= nodes.len() => index = parent,
                    _ => break 0,
                }
            };
            resolved[index as usize].get_or_insert(z);
            for node in chain.drain(..) {
                resolved[node as usize] = Some(z);
            }
        }
        Some(resolved.into_iter().map(|z| z.unwrap_or(0)).collect())
    }

    pub fn runtime_style(&self, index: u32, interaction: InteractionState) -> Style {
        self.runtime_style_cow(index, interaction).into_owned()
    }

    pub fn runtime_style_cow(
        &self,
        index: u32,
        interaction: InteractionState,
    ) -> std::borrow::Cow<'_, Style> {
        let node = &self.document.nodes[index as usize];
        let base_style = interaction.style_for(index, node);
        let has_geometry = self.geometry_overrides.contains_key(&index);
        let has_style = self.style_overrides.contains_key(&index);
        let is_visible = self.is_visible(index);
        let needs_font_adjust =
            self.ui_font_size_offset != 0.0 || (self.ui_font_bold && base_style.font_weight < 600);
        let group = self.group_opacity_factor(index);
        if !has_geometry && !has_style && is_visible && !needs_font_adjust && group == 1.0 {
            return std::borrow::Cow::Borrowed(base_style);
        }
        let mut style = base_style.clone();
        if !is_visible {
            style.display = Display::None;
        }
        if let Some(geometry) = self.geometry_override(index) {
            if let Some(left) = geometry.left {
                style.left = Length::Px(left);
            }
            if let Some(top) = geometry.top {
                style.top = Length::Px(top);
            }
            if let Some(width) = geometry.width {
                style.width = Length::Px(width);
            }
            if let Some(height) = geometry.height {
                style.height = Length::Px(height);
            }
        }
        if let Some(override_style) = self.style_overrides.get(&index) {
            macro_rules! apply {
                ($($field:ident),+) => {
                    $(if let Some(value) = override_style.$field {
                        style.$field = value;
                    })+
                };
            }
            apply!(background, color, border_color, border_radius, opacity);
            apply!(flex_direction, font_size, font_weight);
            apply!(overflow_x, overflow_y);
        }
        style.font_size = (style.font_size + self.ui_font_size_offset).max(6.0);
        if self.ui_font_bold && style.font_weight < 600 {
            style.font_weight = 600;
        }
        style.opacity *= group;
        std::borrow::Cow::Owned(style)
    }

    pub fn text_for(&self, index: u32) -> &str {
        self.text_overrides
            .get(&index)
            .map(String::as_str)
            .unwrap_or_else(|| &*self.document.nodes[index as usize].text)
    }

    pub fn resolve_color(&self, value: ColorValue) -> Color {
        match value {
            ColorValue::Literal(color) => color,
            ColorValue::Variable(index) => self
                .variable_values
                .get(index as usize)
                .copied()
                .unwrap_or(Color::TRANSPARENT),
        }
    }
}
