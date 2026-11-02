//! End-to-end software painting of the non-X11 chain.
//!
//! The Wayland lock presenter cannot use XRender, so this exercises the chain a Wayland
//! presenter actually runs: `PaintCommand` -> `PixelOp` -> RGBA8 pixels. Text arrives as a
//! prepared coverage mask, which is what a software font rasterizer supplies, so no Xft is
//! involved anywhere here.

use flamewm_render_core::lower::{FrameSource, lower_commands};
use flamewm_render_core::soft::SoftPixels;
use flamewm_render_core::{Color, PaintCommand, Rect};

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

/// Paints `commands` into a `width` x `height` frame and returns it.
fn paint(commands: &[PaintCommand], width: u32, height: u32) -> Vec<u8> {
    let ops = lower_commands(commands, 1.0, &[], (width, height), None).expect("commands lower");
    let mut frame = vec![0u8; width as usize * height as usize * 4];
    SoftPixels::new()
        .paint(&mut frame, width, height, &ops)
        .expect("software paint");
    frame
}

fn at(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = (y as usize * width as usize + x as usize) * 4;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

#[test]
fn a_lowered_fill_reaches_the_frame() {
    let commands = [PaintCommand::FillRect {
        rect: rect(4.0, 4.0, 8.0, 8.0),
        color: color(10, 200, 30, 255),
        radius: 0.0,
    }];
    let frame = paint(&commands, 16, 16);
    assert_eq!(at(&frame, 16, 6, 6), [10, 200, 30, 255], "inside the fill");
    assert_eq!(at(&frame, 16, 0, 0), [0, 0, 0, 0], "outside the fill");
}

#[test]
fn a_text_mask_from_a_source_is_blended_without_xft() {
    let commands = [PaintCommand::Text {
        x: 0.0,
        y: 0.0,
        color: color(255, 255, 255, 255),
        size: 12.0,
        weight: 400,
        text: "A".into(),
    }];
    let alpha = [255u8];
    let sources = [FrameSource::Mask {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        alpha: &alpha,
        color: [255, 255, 255, 255],
    }];
    let ops = lower_commands(&commands, 1.0, &sources, (4, 4), None).expect("lower");
    let mut frame = vec![0u8; 4 * 4 * 4];
    SoftPixels::new()
        .paint(&mut frame, 4, 4, &ops)
        .expect("paint");
    assert_eq!(
        &frame[0..4],
        &[255, 255, 255, 255],
        "glyph coverage painted"
    );
}

#[test]
fn a_backdrop_source_is_painted_beneath_the_commands() {
    let backdrop = [7u8, 8, 9, 255];
    let mut frame = vec![0u8; 4];
    let ops = lower_commands(&[], 1.0, &[], (1, 1), Some((1, 1, &backdrop))).expect("lower");
    SoftPixels::new()
        .paint(&mut frame, 1, 1, &ops)
        .expect("paint");
    assert_eq!(frame, vec![7, 8, 9, 255]);
}

#[test]
fn a_frame_is_the_size_a_shm_buffer_needs() {
    // A Wayland presenter hands the compositor exactly this many bytes.
    let (width, height) = (40u32, 30u32);
    let frame = paint(&[], width, height);
    assert_eq!(frame.len(), (width * height * 4) as usize);
}

#[test]
fn a_software_glyph_run_paints_real_text() {
    // The last input a Wayland presenter needs is a text mask without Xft. Rasterize one from an
    // installed font and run it through the same lowering and blending as any other primitive.
    let Some(mask) = flamewm_render_core::glyphs::text_mask("Hi", 32.0, 4.0, 40.0) else {
        // No usable font installed: nothing to assert, and the presenter reports its own fallback.
        return;
    };
    assert!(mask.width > 0 && mask.height > 0, "glyph run has a bitmap");

    let commands = [PaintCommand::Text {
        x: 4.0,
        y: 40.0,
        color: color(255, 255, 255, 255),
        size: 32.0,
        weight: 400,
        text: "Hi".into(),
    }];
    let sources = [FrameSource::Mask {
        x: mask.x,
        y: mask.y,
        width: mask.width,
        height: mask.height,
        alpha: &mask.coverage,
        color: [255, 255, 255, 255],
    }];
    let (width, height) = (96u32, 64u32);
    let ops = lower_commands(&commands, 1.0, &sources, (width, height), None).expect("lower");
    let mut frame = vec![0u8; (width * height * 4) as usize];
    SoftPixels::new()
        .paint(&mut frame, width, height, &ops)
        .expect("paint");

    assert!(
        frame.chunks_exact(4).any(|pixel| pixel[3] > 0),
        "the glyph run must ink at least one pixel"
    );
}
