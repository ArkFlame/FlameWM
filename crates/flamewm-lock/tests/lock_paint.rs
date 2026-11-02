//! Paints the real lock UI through the non-X11 chain.
//!
//! This is the strongest check available without a display: the actual compiled lock template is
//! decoded, projected with a lock view, laid out, lowered and rasterized onto an RGBA8 frame.
//! If any link in the Wayland presenter's paint path broke, this fails — no X server, `XRender` or
//! `Xft` is involved.

use flamewm_lock::document;
use flamewm_lock::secret::PasswordBuffer;
use flamewm_lock::settings::LockSettings;
use flamewm_lock::state::Status;
use flamewm_lock::view::LockView;
use flamewm_render_core::glyphs;
use flamewm_render_core::lower::{FrameSource, lower_commands};
use flamewm_render_core::soft::SoftPixels;
use flamewm_render_core::{
    InteractionState, LayoutEngine, PaintCommand, RuntimeDocument, build_paint_commands,
};
use flamewm_ui_document::UiDocumentView;

/// A prepared source, owned so the borrowed views can be built afterwards.
enum Owned {
    Mask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        coverage: Vec<u8>,
        color: [u8; 4],
    },
    Image {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

impl Owned {
    fn as_source(&self) -> FrameSource<'_> {
        match self {
            Self::Mask {
                x,
                y,
                width,
                height,
                coverage,
                color,
            } => FrameSource::Mask {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                alpha: coverage,
                color: *color,
            },
            Self::Image {
                x,
                y,
                width,
                height,
                rgba,
            } => FrameSource::Image {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                rgba,
            },
        }
    }
}

/// The glyph coverage for one run, at the pen position the painter uses.
fn glyph_source(text: &str, size: f32, x: f32, y: f32, color: [u8; 4]) -> Owned {
    let (mask_x, mask_y, width, height, coverage) = match glyphs::text_mask(text, size, x, y) {
        Some(mask) => (mask.x, mask.y, mask.width, mask.height, mask.coverage),
        // No installed font: an empty mask keeps the command order intact.
        None => (0, 0, 0, 0, Vec::new()),
    };
    Owned::Mask {
        x: mask_x,
        y: mask_y,
        width,
        height,
        coverage,
        color,
    }
}

/// One source per `Text`, `TextShadow` and `Image` command, in command order.
///
/// This is the work a Wayland presenter does in place of `Xft` and the `XRender` image blit.
fn prepare_sources(commands: &[PaintCommand], document: &RuntimeDocument) -> Vec<Owned> {
    let mut sources = Vec::new();
    for command in commands {
        match command {
            PaintCommand::Text {
                x,
                y,
                color,
                size,
                text,
                ..
            } => sources.push(glyph_source(
                text,
                *size,
                *x,
                *y,
                [color.r, color.g, color.b, color.a],
            )),
            PaintCommand::TextShadow {
                x,
                y,
                color,
                size,
                text,
                ..
            } => sources.push(glyph_source(
                text,
                *size,
                *x,
                *y,
                [color.r, color.g, color.b, color.a],
            )),
            PaintCommand::Image { rect, node, .. } => {
                // The document already holds the decoded pixels for this node.
                let (width, height, rgba) = match document.image_for_node(*node) {
                    Some((_asset, image_width, image_height, _revision, pixels)) => {
                        (image_width, image_height, pixels.to_vec())
                    }
                    None => (0, 0, Vec::new()),
                };
                sources.push(Owned::Image {
                    x: rect.x.round() as i32,
                    y: rect.y.round() as i32,
                    width,
                    height,
                    rgba,
                });
            }
            _ => {}
        }
    }
    sources
}

#[test]
fn the_real_lock_ui_paints_to_pixels_without_x11() {
    let settings = LockSettings::default();
    let password = PasswordBuffer::default();
    let view = LockView::project(&settings, None, "self-test", &password, Status::Idle);

    let (width, height) = (1280u32, 800u32);
    let template = document::template().expect("compiled lock template decodes");
    let mut runtime = template.into_runtime_document();
    let background = document::background_image(&settings, width, height);
    {
        // The same document edits the X11 presenter applies.
        let mut access = UiDocumentView::from_runtime(&mut runtime, None);
        document::apply_document(&mut access, &settings, width, height, background)
            .expect("lock document applies");
        document::apply_view(&mut access, &view).expect("lock view applies");
    }

    let layout = LayoutEngine::compute(
        &runtime,
        width as f32,
        height as f32,
        InteractionState::default(),
    );
    let commands = build_paint_commands(&runtime, &layout, InteractionState::default());
    assert!(!commands.is_empty(), "the lock UI must paint something");

    // A software presenter snaps to device pixels: the X11 painter resolves fractional
    // geometry directly, but a shm frame can only hold whole pixels.
    let commands = flamewm_render_core::lower::snap_commands(&commands, 1.0);
    let prepared = prepare_sources(&commands, &runtime);
    let sources: Vec<FrameSource<'_>> = prepared.iter().map(Owned::as_source).collect();

    let ops = lower_commands(&commands, 1.0, &sources, (width, height), None)
        .expect("the lock UI lowers to pixel ops");

    let mut frame = vec![0u8; (width * height * 4) as usize];
    SoftPixels::new()
        .paint(&mut frame, width, height, &ops)
        .expect("the lock UI paints");

    // The lock output must be opaque: nothing of the desktop may show through it.
    let opaque = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255)
        .count();
    assert!(
        opaque > (width * height) as usize / 2,
        "the lock UI must cover most of the output, got {opaque} opaque pixels"
    );
}
