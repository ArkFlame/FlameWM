//! Client GPU buffers: the EGL display bound to this `wl_display` and the `zwp_linux_dmabuf_v1`
//! global with its default feedback.
//!
//! The renderer decides what is offered. Only formats it can import are advertised, and only the
//! RGB formats its compositing path samples are kept, so a client never gets a format the frame
//! path cannot draw. A renderer without import support gets no global and the compositor keeps
//! serving `wl_shm` alone.

use std::os::unix::fs::MetadataExt;

use smithay::backend::allocator::Format;
use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{ImportDma, ImportEgl};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, ImportNotifier};

use crate::state::State;

/// The RGB formats the compositing path samples. Anything else the renderer can import (YUV
/// planes, for example) is left out of the advertised set.
const COMPOSITED_CODES: [Fourcc; 8] = [
    Fourcc::Argb8888,
    Fourcc::Xrgb8888,
    Fourcc::Abgr8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb2101010,
    Fourcc::Xrgb2101010,
    Fourcc::Abgr2101010,
    Fourcc::Xbgr2101010,
];

/// Keeps the formats the compositing path can sample, preserving each one's modifier and order.
pub fn filter_formats(formats: impl IntoIterator<Item = Format>) -> Vec<Format> {
    formats
        .into_iter()
        .filter(|format| COMPOSITED_CODES.contains(&format.code))
        .collect()
}

/// The `dev_t` of a render node. Linux defines it as 64 bits on every supported target.
type DevT = u64;

/// The default feedback: one main tranche on `main_device` with `formats`. Pure apart from the
/// memfd smithay writes the format table into, so it builds without a GPU.
pub fn default_feedback(
    main_device: DevT,
    formats: Vec<Format>,
) -> Result<DmabufFeedback, std::io::Error> {
    DmabufFeedbackBuilder::new(main_device, formats).build()
}

/// The `dev_t` clients must use as the feedback's main device: the render node behind the
/// renderer's EGL display. `None` when the display has no render node (software rasterizers).
fn main_device(renderer: &GlowRenderer) -> Option<DevT> {
    let device = EGLDevice::device_for_display(renderer.egl_context().display()).ok()?;
    let path = device.render_device_path().ok()?;
    std::fs::metadata(path).ok().map(|metadata| metadata.rdev())
}

/// Binds the renderer's EGL display to the compositor's `wl_display` and creates the dmabuf global
/// when the renderer can import what it advertises. Each unsupported step is logged once, and the
/// compositor keeps running on `wl_shm` for everything that cannot be offered.
pub fn init(state: &mut State, renderer: &mut GlowRenderer) {
    if let Err(error) = renderer.bind_wl_display(&state.display) {
        eprintln!(
            "flamewm-wayland: dmabuf egl bind unavailable ({error}); EGL clients fall back to wl_shm"
        );
    }
    let formats = filter_formats(renderer.dmabuf_formats());
    let Some(device) = main_device(renderer) else {
        eprintln!("flamewm-wayland: dmabuf global skipped: renderer has no render node");
        return;
    };
    if formats.is_empty() {
        eprintln!("flamewm-wayland: dmabuf global skipped: renderer imports no RGB formats");
        return;
    }
    let count = formats.len();
    match default_feedback(device, formats) {
        Ok(feedback) => {
            let _global = state
                .dmabuf_state
                .create_global_with_default_feedback::<State>(&state.display, &feedback);
            eprintln!("flamewm-wayland: dmabuf global zwp_linux_dmabuf_v1 formats={count}");
        }
        Err(error) => {
            eprintln!("flamewm-wayland: dmabuf feedback unavailable ({error}); no dmabuf global");
        }
    }
}

/// Answers one client import: a successful import tells the client the buffer is usable, and a
/// failed one rejects it. The texture is dropped at once; the frame path imports each buffer again
/// when it draws it.
pub fn settle_import(renderer: &mut GlowRenderer, dmabuf: &Dmabuf, notifier: ImportNotifier) {
    match renderer.import_dmabuf(dmabuf, None) {
        Ok(_texture) => {
            let _ = notifier.successful::<State>();
        }
        Err(error) => {
            eprintln!("flamewm-wayland: dmabuf import rejected: {error}");
            notifier.failed();
        }
    }
}

/// Settles the imports queued while no renderer was reachable from `State` (native backend).
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn settle_pending(state: &mut State, renderer: &mut GlowRenderer) {
    for (dmabuf, notifier) in std::mem::take(&mut state.pending_dmabufs) {
        settle_import(renderer, &dmabuf, notifier);
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::Modifier;

    use super::{default_feedback, filter_formats};
    use smithay::backend::allocator::{Format, Fourcc};

    fn format(code: Fourcc, modifier: Modifier) -> Format {
        Format { code, modifier }
    }

    #[test]
    fn keeps_only_composited_rgb_codes_in_order() {
        let kept = filter_formats([
            format(Fourcc::Xrgb8888, Modifier::Linear),
            format(Fourcc::Nv12, Modifier::Linear),
            format(Fourcc::Argb8888, Modifier::Invalid),
            format(Fourcc::Yuyv, Modifier::Linear),
            format(Fourcc::Xbgr2101010, Modifier::Linear),
        ]);
        assert_eq!(
            kept,
            vec![
                format(Fourcc::Xrgb8888, Modifier::Linear),
                format(Fourcc::Argb8888, Modifier::Invalid),
                format(Fourcc::Xbgr2101010, Modifier::Linear),
            ]
        );
    }

    #[test]
    fn filter_of_only_unsupported_codes_is_empty() {
        assert!(filter_formats([format(Fourcc::Nv12, Modifier::Linear)]).is_empty());
    }

    #[test]
    fn default_feedback_builds_without_a_gpu() {
        let formats = filter_formats([
            format(Fourcc::Argb8888, Modifier::Linear),
            format(Fourcc::Xrgb8888, Modifier::Linear),
        ]);
        assert!(default_feedback(0x0100_0080, formats).is_ok());
    }
}
