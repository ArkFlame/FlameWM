use std::sync::Arc;

use crate::layout::LayoutResult;
use crate::model::*;

#[derive(Clone, Debug, PartialEq)]
pub enum PaintCommand {
    /// Blurred root pixels beneath a translucent node, painted before the node's fill.
    NodeBackdrop {
        rect: Rect,
        radius: f32,
        blur_percent: u8,
        /// Compositing strength of the node's blurred backdrop; derived from the
        /// effective fill alpha, so a 5% capsule cannot cover the wallpaper opaquely.
        opacity: u8,
        node: u32,
    },
    FillRect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    StrokeRect {
        rect: Rect,
        color: Color,
        width: f32,
        radius: f32,
    },
    Image {
        rect: Rect,
        asset: u16,
        node: u32,
        revision: u64,
        treatment: ImageTreatment,
        tint: Option<Color>,
    },
    Text {
        x: f32,
        y: f32,
        color: Color,
        size: f32,
        weight: u16,
        text: Arc<str>,
    },
    TextShadow {
        x: f32,
        y: f32,
        color: Color,
        size: f32,
        weight: u16,
        blur_radius: f32,
        text: Arc<str>,
    },
    PushClip {
        rect: Rect,
    },
    PopClip,
    ScrollbarTrack {
        rect: Rect,
        horizontal: bool,
    },
    ScrollbarThumb {
        rect: Rect,
        horizontal: bool,
    },
}

/// Viewport clip for a scrollable element: Some(rect) when either axis clips.
pub fn scroll_clip(style: &Style, viewport: Rect) -> Option<Rect> {
    let clips_x = matches!(
        style.overflow_x,
        Overflow::Hidden | Overflow::Auto | Overflow::Scroll
    );
    let clips_y = matches!(
        style.overflow_y,
        Overflow::Hidden | Overflow::Auto | Overflow::Scroll
    );
    (clips_x || clips_y).then_some(viewport)
}

/// Scrollbar visibility for one axis: Scroll forces track, Auto shows only
/// when content overflows, Visible/Hidden never paint scrollbars.
pub fn scrollbar_visible(mode: Overflow, viewport_len: f32, content_len: f32) -> bool {
    match mode {
        Overflow::Scroll => true,
        Overflow::Auto => content_len > viewport_len,
        Overflow::Visible | Overflow::Hidden => false,
    }
}

pub fn build_paint_commands(
    document: &RuntimeDocument,
    layout: &LayoutResult,
    interaction: InteractionState,
) -> Vec<PaintCommand> {
    build_paint_commands_with_scroll(document, layout, interaction, &|_| ScrollState::default())
}

/// Same as [`build_paint_commands`] but offsets scrollable content by the
/// retained per-node scroll state and emits clip + scrollbar commands.
pub fn build_paint_commands_with_scroll(
    document: &RuntimeDocument,
    layout: &LayoutResult,
    interaction: InteractionState,
    scroll: &dyn Fn(u32) -> ScrollState,
) -> Vec<PaintCommand> {
    // Sized by the nodes this call walks, not the whole document: the per-node paint memo calls this once per node,
    // and a document-sized reservation there touched pages per node (about 50 MB for a 6k-node terminal grid).
    let mut commands = Vec::with_capacity(layout.z_order.len() * 2);
    for index in layout.z_order.iter().copied() {
        let index = index as usize;
        let node = &document.document.nodes[index];
        if !document.is_effectively_visible(index as u32) {
            continue;
        }
        let style = document.runtime_style(index as u32, interaction);
        if style.display == Display::None {
            continue;
        }
        let rect = layout
            .boxes
            .get(index)
            .map(|entry| entry.rect)
            .unwrap_or_default();
        if rect.width <= 0.0 || rect.height <= 0.0 {
            continue;
        }
        if node.kind == NodeKind::Element || node.kind == NodeKind::Image {
            let background = apply_opacity(document.resolve_color(style.background), style.opacity);
            if background.a != 0 {
                if let Some(blur_percent) = document.node_backdrop_blur(index as u32) {
                    if background.a < 255 {
                        commands.push(PaintCommand::NodeBackdrop {
                            rect,
                            radius: style.border_radius.max(0.0),
                            blur_percent,
                            opacity: background.a,
                            node: index as u32,
                        });
                    }
                }
                commands.push(PaintCommand::FillRect {
                    rect,
                    color: background,
                    radius: style.border_radius.max(0.0),
                });
            }
            let border = apply_opacity(document.resolve_color(style.border_color), style.opacity);
            if style.border_width > 0.0 && border.a != 0 {
                commands.push(PaintCommand::StrokeRect {
                    rect,
                    color: border,
                    width: style.border_width,
                    radius: style.border_radius.max(0.0),
                });
            }
        }
        match node.kind {
            NodeKind::Image => {
                let node_index = index as u32;
                // Node-local identity: override revision when present; the
                // asset id is the fallback descriptor, never pixel data.
                let asset = node.image.unwrap_or(u16::MAX);
                let revision = document.image_revision_for_node(node_index);
                if document.image_for_node(node_index).is_some() {
                    let (treatment, tint) = match style.image_treatment {
                        ImageTreatment::Original => (ImageTreatment::Original, None),
                        ImageTreatment::SymbolicForeground => {
                            let tint =
                                apply_opacity(document.resolve_color(style.color), style.opacity);
                            (ImageTreatment::SymbolicForeground, Some(tint))
                        }
                    };
                    commands.push(PaintCommand::Image {
                        rect,
                        asset,
                        node: node_index,
                        revision,
                        treatment,
                        tint,
                    });
                }
            }
            NodeKind::Text => {
                let color = apply_opacity(document.resolve_color(style.color), style.opacity);
                if color.a != 0 {
                    let text = document.text_for(index as u32);
                    if !text.is_empty() {
                        let laid = crate::text_layout::layout_text(
                            text,
                            style.font_size,
                            rect.width.max(0.0),
                            style.text_wrap,
                            style.break_anywhere,
                        );
                        for (line_index, line) in laid.lines.into_iter().enumerate() {
                            if line.text.is_empty() {
                                continue;
                            }
                            let y = rect.y + laid.line_height * line_index as f32 + style.font_size;
                            // One allocation per laid-out line; every emitted command shares it by refcount.
                            let text: Arc<str> = Arc::from(line.text);
                            if let Some(shadow) = style.text_shadow {
                                let shade = apply_opacity(
                                    document.resolve_color(shadow.color),
                                    style.opacity,
                                );
                                if shade.a != 0 {
                                    if shadow.blur_radius > 0.0 {
                                        commands.push(PaintCommand::TextShadow {
                                            x: rect.x + shadow.offset_x,
                                            y: y + shadow.offset_y,
                                            color: shade,
                                            size: style.font_size,
                                            weight: style.font_weight,
                                            blur_radius: shadow.blur_radius,
                                            text: Arc::clone(&text),
                                        });
                                    } else {
                                        commands.push(PaintCommand::Text {
                                            x: rect.x + shadow.offset_x,
                                            y: y + shadow.offset_y,
                                            color: shade,
                                            size: style.font_size,
                                            weight: style.font_weight,
                                            text: Arc::clone(&text),
                                        });
                                    }
                                }
                            }
                            commands.push(PaintCommand::Text {
                                x: rect.x,
                                y,
                                color,
                                size: style.font_size,
                                weight: style.font_weight,
                                text,
                            });
                        }
                    }
                }
            }
            NodeKind::Element => {
                emit_scroll_chrome(&mut commands, &style, layout, index as u32, scroll);
            }
        }
    }
    commands
}

/// Emit PushClip/PopClip plus scrollbar track/thumb for a scrollable element.
/// Offsets come from retained per-node state; metrics derive from layout only.
pub fn emit_scroll_chrome(
    commands: &mut Vec<PaintCommand>,
    style: &Style,
    layout: &LayoutResult,
    index: u32,
    scroll: &dyn Fn(u32) -> ScrollState,
) {
    use crate::layout::LayoutResult as LR;
    let viewport = layout
        .boxes
        .get(index as usize)
        .map(|b| b.rect)
        .unwrap_or_default();
    if scroll_clip(style, viewport).is_none() {
        return;
    }
    let content = layout.content_extent(index);
    let offset = scroll(index);
    let metrics = LR::scroll_metrics(viewport, content, offset);
    commands.push(PaintCommand::PushClip { rect: viewport });
    if style.hide_scrollbars {
        commands.push(PaintCommand::PopClip);
        return;
    }
    if scrollbar_visible(style.overflow_x, viewport.width, content.width) {
        let track = Rect {
            x: viewport.x,
            y: viewport.y + viewport.height - 8.0,
            width: viewport.width,
            height: 8.0,
        };
        commands.push(PaintCommand::ScrollbarTrack {
            rect: track,
            horizontal: true,
        });
        if metrics.thumb_x.width > 0.0 {
            commands.push(PaintCommand::ScrollbarThumb {
                rect: metrics.thumb_x,
                horizontal: true,
            });
        }
    }
    if scrollbar_visible(style.overflow_y, viewport.height, content.height) {
        let track = Rect {
            x: viewport.x + viewport.width - 8.0,
            y: viewport.y,
            width: 8.0,
            height: viewport.height,
        };
        commands.push(PaintCommand::ScrollbarTrack {
            rect: track,
            horizontal: false,
        });
        if metrics.thumb_y.height > 0.0 {
            commands.push(PaintCommand::ScrollbarThumb {
                rect: metrics.thumb_y,
                horizontal: false,
            });
        }
    }
    commands.push(PaintCommand::PopClip);
}

fn apply_opacity(mut color: Color, opacity: f32) -> Color {
    let opacity = opacity.clamp(0.0, 1.0);
    color.a = ((color.a as f32) * opacity).round() as u8;
    color
}

/// Root background corner radius for the current interaction state. Top-level
/// rounded popup/menu/dropdown surfaces derive their X Shape bounding mask
/// from this radius plus the surface content bounds.
pub fn root_corner_radius(document: &RuntimeDocument, interaction: InteractionState) -> f32 {
    let root = document.document.root;
    if document.document.nodes.get(root as usize).is_none() {
        return 0.0;
    }
    document
        .runtime_style(root, interaction)
        .border_radius
        .max(0.0)
}

/// Corner radius in px clamped so it never exceeds half of either side.
pub fn clamp_radius_px(width: u32, height: u32, radius: u32) -> u32 {
    radius.min(width / 2).min(height / 2)
}

/// Surface-shape pixels for an X Shape bounding mask: (width, height, radius)
/// clamped so the radius never exceeds half the smallest side.
pub fn surface_shape_pixels(width: u32, height: u32, radius: f32) -> (u32, u32, u32) {
    let clamped = (radius.round().max(0.0) as u32)
        .min(width / 2)
        .min(height / 2);
    (width.max(1), height.max(1), clamped)
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    fn rounded_doc(radius: f32) -> RuntimeDocument {
        let style = Style {
            background: ColorValue::Literal(Color::rgb(30, 30, 30)),
            border_radius: radius,
            ..Style::default()
        };
        let node = CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "root".to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: None,
            style,
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![node],
        })
        .expect("fixture validates")
    }

    #[test]
    fn root_radius_flows_into_surface_shape() {
        let doc = rounded_doc(9.0);
        let radius = root_corner_radius(&doc, InteractionState::default());
        assert_eq!(radius, 9.0);
        assert_eq!(surface_shape_pixels(100, 60, radius), (100, 60, 9));
    }

    #[test]
    fn surface_shape_clamps_radius_to_half_min_side() {
        assert_eq!(surface_shape_pixels(8, 8, 99.0), (8, 8, 4));
        assert_eq!(surface_shape_pixels(0, 0, 4.0), (1, 1, 0));
        assert_eq!(surface_shape_pixels(10, 6, -3.0), (10, 6, 0));
    }
}

#[cfg(test)]
mod node_backdrop_tests {
    use super::*;
    use crate::layout::LayoutEngine;

    fn node(
        id: &str,
        radius: f32,
        parent: Option<u32>,
        first_child: Option<u32>,
        next: Option<u32>,
    ) -> CompiledNode {
        let style = Style {
            background: ColorValue::Literal(Color {
                r: 30,
                g: 30,
                b: 30,
                a: 127,
            }),
            border_radius: radius,
            ..Style::default()
        };
        CompiledNode {
            kind: NodeKind::Element,
            parent,
            first_child,
            next_sibling: next,
            id: id.to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: None,
            style,
            hover_style: None,
            active_style: None,
        }
    }

    #[test]
    fn node_backdrop_radius_is_the_menu_node_radius_not_the_root() {
        // Root (desktop window) radius 0; menu child radius 12, like project_menu_surface.
        let mut root = node("desktop", 0.0, None, Some(1), None);
        root.style.background = ColorValue::Literal(Color::TRANSPARENT);
        let mut document = RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![root, node("menu", 12.0, Some(0), None, None)],
        })
        .expect("fixture validates");
        document.set_size_px("menu", 200.0, 100.0).expect("size");
        document
            .set_position_px("menu", 10.0, 10.0)
            .expect("position");
        document
            .set_node_backdrop("menu", Some(80))
            .expect("backdrop");
        let layout = LayoutEngine::compute(&document, 800.0, 600.0, InteractionState::default());
        let commands = build_paint_commands(&document, &layout, InteractionState::default());
        let radii: Vec<f32> = commands
            .iter()
            .filter_map(|command| match command {
                PaintCommand::NodeBackdrop { radius, .. } => Some(*radius),
                _ => None,
            })
            .collect();
        assert_eq!(radii, vec![12.0]);
    }

    #[test]
    fn translucent_node_backdrop_uses_effective_fill_alpha() {
        // A separate taskbar capsule has an opaque wallpaper underneath it. Its
        // blur must NOT replace the wallpaper with a fully opaque layer at 5%.
        let mut root = node("taskbar", 0.0, None, Some(1), None);
        root.style.background = ColorValue::Literal(Color::TRANSPARENT);
        let mut capsule = node("start-capsule", 8.0, Some(0), None, None);
        capsule.style.background = ColorValue::Literal(Color {
            r: 25,
            g: 27,
            b: 29,
            a: 13,
        });
        let mut document = RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![root, capsule],
        })
        .expect("capsule fixture");
        document.set_size_px("start-capsule", 42.0, 44.0).unwrap();
        document
            .set_node_backdrop("start-capsule", Some(80))
            .unwrap();
        let layout = LayoutEngine::compute(&document, 100.0, 44.0, InteractionState::default());
        let commands = build_paint_commands(&document, &layout, InteractionState::default());
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, PaintCommand::NodeBackdrop { opacity: 13, .. })),
            "5% tint must not get an opaque blur"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                PaintCommand::FillRect {
                    color: Color { a: 13, .. },
                    ..
                }
            )),
            "tint alpha must remain unchanged"
        );
    }

    /// Real-Start-shaped scroll fixture: the 100x50 `pane` is the sized box,
    /// the `list` inside it is stretched to that box by the flex parent (no
    /// explicit height of its own) and holds one 100x200 row, so the retained
    /// content extent (200) genuinely overflows the 100x50 viewport.
    ///
    /// Two fixture traps this avoids:
    /// - `LayoutEngine::compute` forces the *root* to the viewport size, so a
    ///   root scroller can never overflow a smaller requested size.
    /// - A scroller with its own explicit `height`/`max-height` measures its
    ///   content extent through that same clamp, so the overflow would be
    ///   invisible (`app/scroll.rs::fit_scroll_extents` documents the same
    ///   engine limitation). Real panes (`.start-content-pane`) take their box
    ///   from the parent and keep the scroller's extent content-driven.
    fn scroll_list(hide_scrollbars: bool) -> Vec<PaintCommand> {
        let mut root = node("root", 0.0, None, Some(1), None);
        root.style.background = ColorValue::Literal(Color::TRANSPARENT);
        let mut pane = node("pane", 0.0, Some(0), Some(2), None);
        pane.style.display = Display::Flex;
        pane.style.flex_direction = FlexDirection::Row;
        let mut list = node("list", 0.0, Some(1), Some(3), None);
        list.style.overflow_y = Overflow::Auto;
        list.style.hide_scrollbars = hide_scrollbars;
        let mut document = RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![root, pane, list, node("row", 0.0, Some(2), None, None)],
        })
        .expect("fixture validates");
        document.set_size_px("pane", 100.0, 50.0).expect("size");
        document.set_size_px("row", 100.0, 200.0).expect("size");
        let layout = LayoutEngine::compute(&document, 800.0, 600.0, InteractionState::default());
        // Guard the fixture itself: the scroller is the 100x50 list and its
        // content extent really is 200px tall.
        let viewport = layout.boxes[2].rect;
        assert_eq!((viewport.width, viewport.height), (100.0, 50.0));
        assert_eq!(layout.content_extent(2).height, 200.0);
        build_paint_commands_with_scroll(&document, &layout, InteractionState::default(), &|_| {
            ScrollState {
                offset_x: 0.0,
                offset_y: 10.0,
            }
        })
    }

    #[test]
    fn hidden_scrollbars_paint_no_track_or_thumb_but_keep_the_clip() {
        let visible = scroll_list(false);
        let tracks: Vec<&PaintCommand> = visible
            .iter()
            .filter(|c| matches!(c, PaintCommand::ScrollbarTrack { .. }))
            .collect();
        assert!(!tracks.is_empty(), "overflowing content must emit a track");
        assert!(
            visible
                .iter()
                .any(|c| matches!(c, PaintCommand::ScrollbarThumb { .. }))
        );
        // Only the vertical axis overflows, so the only track is the 8px
        // right-edge strip of the 100x50 viewport.
        assert_eq!(tracks.len(), 1, "{tracks:?}");
        let (track_rect, horizontal) = match tracks[0] {
            PaintCommand::ScrollbarTrack { rect, horizontal } => (*rect, *horizontal),
            _ => unreachable!("filtered to ScrollbarTrack"),
        };
        assert_eq!(
            track_rect,
            Rect {
                x: 92.0,
                y: 0.0,
                width: 8.0,
                height: 50.0,
            }
        );
        assert!(!horizontal);

        let hidden = scroll_list(true);
        assert!(!hidden.iter().any(|c| matches!(
            c,
            PaintCommand::ScrollbarTrack { .. } | PaintCommand::ScrollbarThumb { .. }
        )));
        // Clip still wraps the scrolled content, so rows keep scrolling under it.
        assert!(
            hidden
                .iter()
                .any(|c| matches!(c, PaintCommand::PushClip { .. }))
        );
        assert!(hidden.iter().any(|c| matches!(c, PaintCommand::PopClip)));
    }
}
