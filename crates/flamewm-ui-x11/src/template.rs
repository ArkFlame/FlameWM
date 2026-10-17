use flamewm_render_core::RuntimeDocument;
use flamewm_ui_core::style::UiLayer;

pub struct UiDocument {
    pub(crate) document: RuntimeDocument,
}

pub struct UiDocumentView<'a> {
    pub(crate) document: &'a mut RuntimeDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeImage {
    pub source: String,
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA8 in row-major order. Dynamic/app
    /// icons carry real alpha here; no magenta color-key is consulted.
    pub pixels: Vec<u8>,
}

impl RuntimeImage {
    #[must_use]
    pub fn is_fully_opaque(&self) -> bool {
        self.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255)
    }
}

pub trait UiDocumentAccess {
    fn text(&mut self, id: &str, text: String) -> Result<(), String>;
    fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String>;
    fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String>;
    /// Restores the node's compiled (packaged) image. Documents without image overrides ignore it.
    fn image_reset(&mut self, id: &str) -> Result<(), String> {
        let _ = id;
        Ok(())
    }
    fn visible(&mut self, id: &str, visible: bool) -> Result<(), String>;
    fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String>;
    fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String>;
    fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String>;
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String>;
    fn border(&mut self, id: &str, color: UiColor) -> Result<(), String>;
    /// Corner radius in px for a surface root. Documents without shape support ignore it.
    fn corner_radius(&mut self, id: &str, radius_px: f32) -> Result<(), String> {
        let _ = (id, radius_px);
        Ok(())
    }
    /// Blurred root backdrop under a translucent node (`Some(0)` or `None` clears). Documents
    /// without node backdrop support ignore it.
    fn node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        let _ = (id, blur_percent);
        Ok(())
    }
    fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String>;
    fn background_clear(&mut self, id: &str) -> Result<(), String>;
    fn border_clear(&mut self, id: &str) -> Result<(), String>;
    fn foreground_clear(&mut self, id: &str) -> Result<(), String>;
    fn font_size(&mut self, id: &str, size_px: f32) -> Result<(), String>;
    fn font_weight(&mut self, id: &str, weight: u16) -> Result<(), String>;
    fn font_family(&mut self, family: String) -> Result<(), String> {
        let _ = family;
        Err("font family projection is unsupported by this document access".to_string())
    }
    fn color_variable(&mut self, name: &str, color: UiColor) -> Result<(), String> {
        let _ = (name, color);
        Err("color variable projection is unsupported by this document access".to_string())
    }
    fn overflow(
        &mut self,
        id: &str,
        x: flamewm_render_core::Overflow,
        y: flamewm_render_core::Overflow,
    ) -> Result<(), String>;
    fn layer(&mut self, id: &str, layer: UiLayer) -> Result<(), String>;
}

impl UiDocument {
    /// Layout box `(x, y, width, height)` of a node by id, laid out in a CSS
    /// viewport of `viewport` (the same viewport the render path lays out at).
    /// Read-only: owned by `flamewm_render_core::LayoutEngine`. `None` when the
    /// id is absent from the template.
    pub fn layout_box_by_id(&self, id: &str, viewport: (f32, f32)) -> Option<(f32, f32, f32, f32)> {
        let index = self.document.node_by_id(id)?;
        let layout = flamewm_render_core::LayoutEngine::compute(
            &self.document,
            viewport.0,
            viewport.1,
            flamewm_render_core::InteractionState::default(),
        );
        let rect = layout.boxes.get(index as usize)?.rect;
        Some((rect.x, rect.y, rect.width, rect.height))
    }

    pub fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        self.document.set_text(id, text)
    }

    pub fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        self.document.set_visible(id, visible)
    }

    pub fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        self.image_rgba8(id, image)
    }

    pub fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        replace_rgba8(&mut self.document, id, image)
    }

    pub fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
        set_toggle(&mut self.document, track_id, knob_id, checked)
    }

    pub fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        self.document.set_position_px(id, left, top)
    }

    pub fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        self.document.set_size_px(id, width, height)
    }

    pub fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.document.set_background_color(id, color.into())
    }

    pub fn border(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.document.set_border_color(id, color.into())
    }

    pub fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.document
            .set_foreground_color(id, flamewm_render_core::ColorValue::Literal(color.into()))
    }

    pub fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.document.clear_background_color(id)
    }

    pub fn border_clear(&mut self, id: &str) -> Result<(), String> {
        self.document.clear_border_color(id)
    }

    pub fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
        self.document.clear_foreground_color(id)
    }

    pub fn overflow(
        &mut self,
        id: &str,
        x: flamewm_render_core::Overflow,
        y: flamewm_render_core::Overflow,
    ) -> Result<(), String> {
        self.document.set_overflow(id, x, y)
    }

    pub fn font_size(&mut self, id: &str, size_px: f32) -> Result<(), String> {
        self.document.set_font_size_px(id, size_px)
    }

    pub fn font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
        self.document.set_font_weight(id, weight)
    }

    pub fn font_family(&mut self, family: String) -> Result<(), String> {
        self.document.set_ui_font_family(family)
    }

    pub fn layer(&mut self, id: &str, layer: UiLayer) -> Result<(), String> {
        self.document.set_z_index(id, layer.z_index())
    }
}

/// Shared `UiDocumentAccess` method bodies. `self.document` is either an owned
/// `RuntimeDocument` or a `&mut RuntimeDocument`; both auto-deref to the same calls.
macro_rules! document_access_methods {
    () => {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.document.set_text(id, text)
        }

        fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
            replace_rgba8(&mut self.document, id, image)
        }

        fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
            replace_rgba8(&mut self.document, id, image)
        }

        fn image_reset(&mut self, id: &str) -> Result<(), String> {
            self.document.clear_image_for_node(id)
        }

        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.document.set_visible(id, visible)
        }

        fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
            set_toggle(&mut self.document, track_id, knob_id, checked)
        }

        fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
            self.document.set_position_px(id, left, top)
        }

        fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
            self.document.set_size_px(id, width, height)
        }

        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.document.set_background_color(id, color.into())
        }

        fn border(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.document.set_border_color(id, color.into())
        }

        fn corner_radius(&mut self, id: &str, radius_px: f32) -> Result<(), String> {
            self.document.set_border_radius(id, radius_px)
        }

        fn node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
            self.document.set_node_backdrop(id, blur_percent)
        }

        fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.document
                .set_foreground_color(id, flamewm_render_core::ColorValue::Literal(color.into()))
        }

        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.document.clear_background_color(id)
        }

        fn border_clear(&mut self, id: &str) -> Result<(), String> {
            self.document.clear_border_color(id)
        }

        fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
            self.document.clear_foreground_color(id)
        }

        fn overflow(
            &mut self,
            id: &str,
            x: flamewm_render_core::Overflow,
            y: flamewm_render_core::Overflow,
        ) -> Result<(), String> {
            self.document.set_overflow(id, x, y)
        }

        fn font_size(&mut self, id: &str, size_px: f32) -> Result<(), String> {
            self.document.set_font_size_px(id, size_px)
        }

        fn font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
            self.document.set_font_weight(id, weight)
        }

        fn font_family(&mut self, family: String) -> Result<(), String> {
            self.document.set_ui_font_family(family)
        }

        fn color_variable(&mut self, name: &str, color: UiColor) -> Result<(), String> {
            self.document.set_color_variable(name, color.into())
        }

        fn layer(&mut self, id: &str, layer: UiLayer) -> Result<(), String> {
            self.document.set_z_index(id, layer.z_index())
        }
    };
}

impl UiDocumentAccess for UiDocument {
    document_access_methods!();
}

impl<'a> UiDocumentAccess for UiDocumentView<'a> {
    document_access_methods!();
}

impl From<UiColor> for flamewm_render_core::Color {
    fn from(color: UiColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

fn rgb(value: u32) -> flamewm_render_core::Color {
    flamewm_render_core::Color::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn replace_rgba8(
    document: &mut RuntimeDocument,
    id: &str,
    image: RuntimeImage,
) -> Result<(), String> {
    validate_rgba8(&image)?;
    document.replace_image_for_node(id, image.source, image.width, image.height, image.pixels)
}

fn set_toggle(
    document: &mut RuntimeDocument,
    track_id: &str,
    knob_id: &str,
    checked: bool,
) -> Result<(), String> {
    let recipe = flamewm_skin::recipes::toggle::TOGGLE;
    let color = if checked { recipe.on } else { recipe.off };
    document.set_background_color(track_id, rgb(color.0))?;
    let left = if checked {
        recipe.width - recipe.knob - 2
    } else {
        2
    };
    document.set_position_px(knob_id, left as f32, 2.0)
}

fn validate_rgba8(image: &RuntimeImage) -> Result<(), String> {
    if image.width == 0 || image.height == 0 {
        return Err("runtime image dimensions must be non-zero".to_string());
    }
    let expected = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "runtime image dimensions overflow".to_string())?;
    if image.pixels.len() != expected {
        return Err(format!(
            "runtime image has {} bytes; expected {expected} (straight RGBA8)",
            image.pixels.len()
        ));
    }
    Ok(())
}

/// Visible-shape mask geometry for a rounded popup/menu surface: row spans
/// (y, x_start, x_end_exclusive) in surface pixels, cut from the root
/// background radius plus content bounds. Render-side copy used by the
/// render-owned XShape bounding-mask bridge; kept in sync with the
/// identical pure function in `flamewm-render-core::paint` so the geometry
/// can be verified without a display.
#[cfg(test)]
pub fn rounded_rect_mask_spans(width: u32, height: u32, radius: u32) -> Vec<(u32, u32, u32)> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let radius = flamewm_render_core::paint::clamp_radius_px(width, height, radius);
    if radius <= 1 {
        return (0..height).map(|row| (row, 0, width)).collect();
    }
    let mut spans = Vec::with_capacity(height as usize);
    for row in 0..height {
        let corner_row = row.min(height - 1 - row);
        let inset = if corner_row >= radius {
            0
        } else {
            let r = f64::from(radius);
            let dy = r - (f64::from(corner_row) + 0.5);
            let dx = (r * r - dy * dy).max(0.0).sqrt();
            // Floor matches the render-side corner rasterization: mask spans
            // and painted pixels use one formula.
            (r - dx).floor().clamp(0.0, r) as u32
        };
        let start = inset.min(width);
        let end = width.saturating_sub(inset).max(start);
        spans.push((row, start, end));
    }
    spans
}

pub fn decode_document(bytes: &[u8]) -> Result<UiDocument, String> {
    Ok(UiDocument {
        document: RuntimeDocument::new(flamewm_render_core::decode(bytes)?)?,
    })
}

/// A renderer document kept behind the UI/X11 boundary.
pub struct UiTemplate {
    pub(crate) document: RuntimeDocument,
}

impl UiTemplate {
    pub fn new(document: UiDocument) -> Self {
        Self {
            document: document.document,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_node_doc() -> UiDocument {
        let asset = flamewm_render_core::ImageAsset {
            source: "old".to_string(),
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        };
        let node = flamewm_render_core::CompiledNode {
            kind: flamewm_render_core::NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "icon".to_string(),
            action: String::new(),
            text: String::new(),
            image: Some(0),
            style: flamewm_render_core::Style::default(),
            hover_style: None,
            active_style: None,
        };
        UiDocument {
            document: RuntimeDocument::new(flamewm_render_core::CompiledDocument {
                source_fingerprint: 0,
                root: 0,
                variables: Vec::new(),
                assets: vec![asset],
                nodes: vec![node],
            })
            .expect("fixture validates"),
        }
    }

    #[test]
    fn rgba8_replace_keeps_alpha_and_rejects_rgb8_length() {
        let mut doc = image_node_doc();
        doc.image_rgba8(
            "icon",
            RuntimeImage {
                source: "t".to_string(),
                width: 1,
                height: 1,
                pixels: vec![200, 30, 10, 128],
            },
        )
        .expect("rgba replace");
        // Node-local override wins; compiled asset stays immutable.
        let (_, _, _, _, pixels) = doc.document.image_for_node(0).expect("override present");
        assert_eq!(pixels, &[200, 30, 10, 128]);
        assert_eq!(
            doc.document.document.assets[0].pixels,
            vec![0, 0, 0, 255],
            "compiled asset must stay immutable"
        );
        assert_eq!(doc.document.image_revision_for_node(0), 1);
        let error = doc
            .image_rgba8(
                "icon",
                RuntimeImage {
                    source: "bad".to_string(),
                    width: 1,
                    height: 1,
                    pixels: vec![1, 2, 3],
                },
            )
            .expect_err("3-byte RGB8 must fail");
        assert!(error.contains("straight RGBA8"), "unexpected: {error}");
    }

    #[test]
    fn magenta_pixel_is_opaque_content_not_transparency() {
        let mut doc = image_node_doc();
        doc.image_rgba8(
            "icon",
            RuntimeImage {
                source: "m".to_string(),
                width: 1,
                height: 1,
                pixels: vec![255, 0, 255, 255],
            },
        )
        .expect("magenta replace");
        let (_, _, _, _, pixels) = doc.document.image_for_node(0).expect("override present");
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
        assert!(doc.document.document.assets[0].is_fully_opaque());
    }

    fn shared_placeholder_doc() -> UiDocument {
        let asset = flamewm_render_core::ImageAsset {
            source: "shared".to_string(),
            width: 1,
            height: 1,
            pixels: vec![9, 9, 9, 255],
        };
        let mk = |id: &str| {
            let mut style = flamewm_render_core::Style::default();
            style.width = flamewm_render_core::Length::Px(10.0);
            style.height = flamewm_render_core::Length::Px(10.0);
            flamewm_render_core::CompiledNode {
                kind: flamewm_render_core::NodeKind::Image,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: id.to_string(),
                action: String::new(),
                text: String::new(),
                image: Some(0),
                style,
                hover_style: None,
                active_style: None,
            }
        };
        fn with_parent(
            mut node: flamewm_render_core::CompiledNode,
            parent: Option<u32>,
            next: Option<u32>,
        ) -> flamewm_render_core::CompiledNode {
            node.parent = parent;
            node.next_sibling = next;
            node
        }
        UiDocument {
            document: RuntimeDocument::new(flamewm_render_core::CompiledDocument {
                source_fingerprint: 0,
                root: 0,
                variables: Vec::new(),
                assets: vec![asset],
                nodes: vec![
                    flamewm_render_core::CompiledNode {
                        kind: flamewm_render_core::NodeKind::Element,
                        parent: None,
                        first_child: Some(1),
                        next_sibling: None,
                        id: "root".to_string(),
                        action: String::new(),
                        text: String::new(),
                        image: None,
                        style: flamewm_render_core::Style::default(),
                        hover_style: None,
                        active_style: None,
                    },
                    with_parent(mk("a"), Some(0), Some(2)),
                    with_parent(mk("b"), Some(0), None),
                ],
            })
            .expect("fixture validates"),
        }
    }

    #[test]
    fn shared_placeholder_nodes_keep_distinct_images() {
        let mut doc = shared_placeholder_doc();
        doc.image_rgba8(
            "a",
            RuntimeImage {
                source: "red".to_string(),
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 255],
            },
        )
        .expect("replace a");
        doc.image_rgba8(
            "b",
            RuntimeImage {
                source: "green".to_string(),
                width: 1,
                height: 1,
                pixels: vec![0, 255, 0, 255],
            },
        )
        .expect("replace b");
        let a = doc.document.node_by_id("a").expect("a index");
        let b = doc.document.node_by_id("b").expect("b index");
        let (_, _, _, _, pa) = doc.document.image_for_node(a).expect("a image");
        let (_, _, _, _, pb) = doc.document.image_for_node(b).expect("b image");
        assert_eq!(pa, &[255, 0, 0, 255]);
        assert_eq!(pb, &[0, 255, 0, 255]);
        // Compiled shared asset stays untouched: per-node overrides only.
        assert_eq!(doc.document.document.assets[0].pixels, vec![9, 9, 9, 255]);
        assert_eq!(doc.document.image_revision_for_node(a), 1);
        assert_eq!(doc.document.image_revision_for_node(b), 1);
    }

    #[test]
    fn update_one_shared_node_keeps_sibling_and_advances_revision() {
        let mut doc = shared_placeholder_doc();
        doc.image_rgba8(
            "a",
            RuntimeImage {
                source: "red".to_string(),
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 255],
            },
        )
        .expect("replace a");
        doc.image_rgba8(
            "b",
            RuntimeImage {
                source: "green".to_string(),
                width: 1,
                height: 1,
                pixels: vec![0, 255, 0, 255],
            },
        )
        .expect("replace b");
        doc.image_rgba8(
            "a",
            RuntimeImage {
                source: "blue".to_string(),
                width: 1,
                height: 1,
                pixels: vec![0, 0, 255, 255],
            },
        )
        .expect("update a");
        let a = doc.document.node_by_id("a").expect("a index");
        let b = doc.document.node_by_id("b").expect("b index");
        let (_, _, _, _, pa) = doc.document.image_for_node(a).expect("a image");
        let (_, _, _, _, pb) = doc.document.image_for_node(b).expect("b image");
        assert_eq!(pa, &[0, 0, 255, 255]);
        assert_eq!(pb, &[0, 255, 0, 255], "sibling override must survive");
        assert_eq!(doc.document.image_revision_for_node(a), 2);
        assert_eq!(doc.document.image_revision_for_node(b), 1);
    }

    #[test]
    fn shared_placeholder_paint_commands_carry_per_node_revision() {
        let mut doc = shared_placeholder_doc();
        doc.image_rgba8(
            "a",
            RuntimeImage {
                source: "red".to_string(),
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 255],
            },
        )
        .expect("replace a");
        doc.image_rgba8(
            "b",
            RuntimeImage {
                source: "green".to_string(),
                width: 1,
                height: 1,
                pixels: vec![0, 255, 0, 255],
            },
        )
        .expect("replace b");
        doc.image_rgba8(
            "a",
            RuntimeImage {
                source: "blue".to_string(),
                width: 1,
                height: 1,
                pixels: vec![0, 0, 255, 255],
            },
        )
        .expect("update a");
        let layout = flamewm_render_core::LayoutEngine::compute(
            &doc.document,
            50.0,
            50.0,
            flamewm_render_core::InteractionState::default(),
        );
        let commands = flamewm_render_core::build_paint_commands(
            &doc.document,
            &layout,
            flamewm_render_core::InteractionState::default(),
        );
        let mut revisions: Vec<(u32, u64)> = commands
            .iter()
            .filter_map(|command| match command {
                flamewm_render_core::PaintCommand::Image { node, revision, .. } => {
                    Some((*node, *revision))
                }
                _ => None,
            })
            .collect();
        revisions.sort();
        let a = doc.document.node_by_id("a").expect("a index");
        let b = doc.document.node_by_id("b").expect("b index");
        assert_eq!(revisions, vec![(a, 2), (b, 1)]);
    }

    #[test]
    fn rounded_mask_geometry_matches_quarter_circle_cut() {
        // Floor of (r - dx) at pixel-row centers, matching the render-side
        // corner rasterization. 8x8 r=4: row 0 cuts 2px per side, rows 1..6
        // are full width under floor, row 7 mirrors row 0.
        let spans = rounded_rect_mask_spans(8, 8, 4);
        assert_eq!(
            spans,
            vec![
                (0, 2, 6),
                (1, 0, 8),
                (2, 0, 8),
                (3, 0, 8),
                (4, 0, 8),
                (5, 0, 8),
                (6, 0, 8),
                (7, 2, 6),
            ]
        );
    }

    #[test]
    fn rounded_mask_degrades_to_full_rect_for_small_radius() {
        assert_eq!(rounded_rect_mask_spans(4, 2, 0), vec![(0, 0, 4), (1, 0, 4)]);
        assert!(rounded_rect_mask_spans(0, 8, 4).is_empty());
    }

    fn el_color(
        id: &str,
        parent: Option<u32>,
        color: flamewm_render_core::Color,
    ) -> flamewm_render_core::CompiledNode {
        flamewm_render_core::CompiledNode {
            kind: flamewm_render_core::NodeKind::Element,
            parent,
            first_child: None,
            next_sibling: None,
            id: id.to_string(),
            action: "a".to_string(),
            text: String::new(),
            image: None,
            style: flamewm_render_core::Style {
                background: flamewm_render_core::ColorValue::Literal(color),
                ..flamewm_render_core::Style::default()
            },
            hover_style: None,
            active_style: None,
        }
    }

    fn stacked_doc() -> (UiDocument, flamewm_render_core::LayoutResult) {
        let nodes = vec![
            el_color("base", None, flamewm_render_core::Color::rgb(10, 20, 30)),
            el_color("top", None, flamewm_render_core::Color::rgb(200, 30, 40)),
        ];
        let mut doc = UiDocument {
            document: RuntimeDocument::new(flamewm_render_core::CompiledDocument {
                source_fingerprint: 0,
                root: 0,
                variables: Vec::new(),
                assets: Vec::new(),
                nodes,
            })
            .expect("fixture validates"),
        };
        doc.layer("base", UiLayer::Content).expect("base layer");
        doc.layer("top", UiLayer::Popover).expect("top layer");
        let base = doc.document.node_by_id("base").expect("base index");
        let top = doc.document.node_by_id("top").expect("top index");
        assert_eq!(doc.document.effective_z_index(base), 0);
        assert_eq!(doc.document.effective_z_index(top), 1000);
        let mut boxes =
            vec![flamewm_render_core::LayoutBox::default(); doc.document.document.nodes.len()];
        for entry in &mut boxes {
            entry.rect = flamewm_render_core::Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            };
        }
        let count = doc.document.document.nodes.len();
        let layout = flamewm_render_core::LayoutResult {
            boxes,
            contents: vec![flamewm_render_core::Rect::default(); count],
            revision: doc.document.revision(),
            z_order: {
                let mut order: Vec<u32> = (0..count as u32).collect();
                order.sort_by_key(|index| (doc.document.effective_z_index(*index), *index as i64));
                order
            },
        };
        (doc, layout)
    }

    #[test]
    fn layer_mutation_reaches_effective_z_index() {
        let (mut doc, _) = stacked_doc();
        let top = doc.document.node_by_id("top").expect("top index");
        assert_eq!(
            doc.document.effective_z_index(top),
            UiLayer::Popover.z_index()
        );
        doc.layer("top", UiLayer::Background).expect("relayer");
        assert_eq!(
            doc.document.effective_z_index(top),
            UiLayer::Background.z_index()
        );
    }

    #[test]
    fn paint_order_follows_layer() {
        let (doc, layout) = stacked_doc();
        let commands = flamewm_render_core::build_paint_commands(
            &doc.document,
            &layout,
            flamewm_render_core::InteractionState::default(),
        );
        let base_bg = flamewm_render_core::Color::rgb(10, 20, 30);
        let top_bg = flamewm_render_core::Color::rgb(200, 30, 40);
        let fills: Vec<flamewm_render_core::Color> = commands
            .iter()
            .filter_map(|command| match command {
                flamewm_render_core::PaintCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(fills, vec![base_bg, top_bg]);
    }

    #[test]
    fn hit_order_follows_layer() {
        let (doc, layout) = stacked_doc();
        let top = doc.document.node_by_id("top").expect("top index");
        assert_eq!(layout.hit_test_action(&doc.document, 5.0, 5.0), Some(top));
    }

    #[test]
    fn font_family_projection_reaches_runtime_document() {
        let mut doc = image_node_doc();
        doc.font_family("Noto Sans".to_string())
            .expect("font family projection");
        assert_eq!(doc.document.ui_font_family(), "Noto Sans");
        assert!(doc.font_family(String::new()).is_err());
    }

    #[test]
    fn color_variable_projection_reaches_runtime_document() {
        let default = flamewm_render_core::Color::rgb(0xef, 0x40, 0x48);
        let mut doc = UiDocument {
            document: RuntimeDocument::new(flamewm_render_core::CompiledDocument {
                source_fingerprint: 0,
                root: 0,
                variables: vec![flamewm_render_core::ColorVariable {
                    name: "--accent".to_string(),
                    default,
                }],
                assets: Vec::new(),
                nodes: vec![el_color("root", None, default)],
            })
            .expect("fixture validates"),
        };
        UiDocumentAccess::color_variable(
            &mut doc,
            "--accent",
            UiColor {
                r: 1,
                g: 2,
                b: 3,
                a: 77,
            },
        )
        .expect("variable projection");
        assert_eq!(
            doc.document.variable_values[0],
            flamewm_render_core::Color {
                r: 1,
                g: 2,
                b: 3,
                a: 77
            }
        );
        assert!(
            UiDocumentAccess::color_variable(
                &mut doc,
                "--missing",
                UiColor {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0
                }
            )
            .is_err()
        );
    }
}
