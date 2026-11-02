//! Paints the real lock UI through the product's own non-X11 presenter.
//!
//! This is the strongest check available without a display: the actual compiled lock template is
//! decoded, projected, laid out and rasterized by `flamewm_lock::present`, then inspected. If any
//! link in the Wayland presenter's paint path broke, this fails — no X server, `XRender` or `Xft`
//! is involved.

use flamewm_lock::document;
use flamewm_lock::present::{Frame, paint_document};
use flamewm_lock::secret::PasswordBuffer;
use flamewm_lock::settings::LockSettings;
use flamewm_lock::state::Status;
use flamewm_lock::view::LockView;
use flamewm_ui_document::UiDocumentView;

fn painted_lock(width: u32, height: u32) -> Frame {
    let settings = LockSettings::default();
    let password = PasswordBuffer::default();
    let view = LockView::project(&settings, None, "self-test", &password, Status::Idle);

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
    paint_document(&runtime, width, height, 1.0).expect("the lock UI paints")
}

fn opaque_pixels(frame: &Frame) -> usize {
    frame
        .pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255)
        .count()
}

#[test]
fn the_real_lock_ui_paints_to_pixels_without_x11() {
    let (width, height) = (1280u32, 800u32);
    let frame = painted_lock(width, height);

    assert_eq!(frame.width, width);
    assert_eq!(frame.height, height);
    assert_eq!(
        frame.pixels.len(),
        (width * height * 4) as usize,
        "one RGBA quad per pixel"
    );

    // The lock output must be opaque: nothing of the desktop may show through it.
    let opaque = opaque_pixels(&frame);
    assert!(
        opaque > (width * height) as usize / 2,
        "the lock UI must cover most of the output, got {opaque} opaque pixels"
    );
}

#[test]
fn the_lock_ui_paints_at_a_different_output_size() {
    // Outputs vary; the same document must produce a full-size frame for another geometry.
    let (width, height) = (1920u32, 1080u32);
    let frame = painted_lock(width, height);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    let opaque = opaque_pixels(&frame);
    assert!(
        opaque > (width * height) as usize / 2,
        "a larger output must still be covered, got {opaque} opaque pixels"
    );
}
