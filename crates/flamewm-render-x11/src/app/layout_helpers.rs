//! Native layout helpers: node_global_rect + document_intrinsic_size
//! reading the retained layout stored in X11App. Device-local rects are in
//! window pixels (layout box scaled by document ui_scale); root-space rects
//! add the retained native window origin on top (owned by SurfaceController).

use super::*;

/// Pure scale of one layout box into device pixels.
pub(crate) fn scaled_box_rect(layout: &LayoutResult, scale: f32, index: u32) -> Option<Rect> {
    layout.boxes.get(index as usize).map(|b| Rect {
        x: b.rect.x * scale,
        y: b.rect.y * scale,
        width: b.rect.width * scale,
        height: b.rect.height * scale,
    })
}

impl X11App {
    /// Window-pixel rect of a node from retained layout (scaled by document ui_scale).
    pub(crate) fn node_global_rect(&self, document: &RuntimeDocument, index: u32) -> Option<Rect> {
        let scale = document.ui_scale();
        let layout = self.layout.as_ref()?;
        scaled_box_rect(layout, scale, index)
    }

    /// Intrinsic content size of the document root in device pixels.
    pub(crate) fn document_intrinsic_size(&self, document: &RuntimeDocument) -> Option<(f32, f32)> {
        let scale = document.ui_scale();
        let layout = self.layout.as_ref()?;
        let root = document.document.root as usize;
        let content = layout
            .contents
            .get(root)
            .copied()
            .unwrap_or_else(|| layout.boxes.get(root).map(|b| b.rect).unwrap_or_default());
        Some((content.width * scale, content.height * scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_with_box(rect: Rect) -> LayoutResult {
        LayoutResult {
            boxes: vec![flamewm_render_core::LayoutBox { rect }],
            contents: vec![rect],
            revision: 1,
            z_order: vec![0],
        }
    }

    #[test]
    fn scaled_box_rect_applies_ui_scale() {
        let layout = layout_with_box(Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        });
        assert_eq!(
            scaled_box_rect(&layout, 1.0, 0),
            Some(Rect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            })
        );
        assert_eq!(
            scaled_box_rect(&layout, 1.5, 0),
            Some(Rect {
                x: 15.0,
                y: 30.0,
                width: 150.0,
                height: 75.0,
            })
        );
        assert_eq!(scaled_box_rect(&layout, 1.0, 7), None);
    }
}
