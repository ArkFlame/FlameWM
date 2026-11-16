use smithay::backend::renderer::glow::GlowRenderer;

/// True for Mesa's CPU rasterizers: GLES works but nothing is hardware accelerated.
pub fn is_software_renderer(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["llvmpipe", "softpipe", "swrast", "software rasterizer"]
        .iter()
        .any(|soft| lower.contains(soft))
}

/// Set once a driver refuses `EGL_BUFFER_AGE_EXT`: every later frame repaints in full.
pub(super) static BUFFER_AGE_REFUSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set after the first successful submit, when the window surface is bound and presented.
pub(super) static FIRST_FRAME_SUBMITTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the buffer-age query may run: only once a frame was submitted and no driver refused it.
pub(crate) const fn buffer_age_probe_allowed(first_frame_submitted: bool, refused: bool) -> bool {
    first_frame_submitted && !refused
}

/// Whether the buffer-age probe is skipped from the start: a software renderer never answers it.
pub fn buffer_age_unsupported(renderer_name: &str) -> bool {
    is_software_renderer(renderer_name)
}

/// Whether the buffer-age probe is skipped from the start on this backend. The nested winit EGL
/// surface answers `EGL_BAD_SURFACE` to `BUFFER_AGE` on real Intel GL too (reproduced on
/// `Mesa Intel(R) UHD Graphics (TGL GT1)`, one ERROR per first redraw), so nested sessions always
/// repaint in full: age 0 is the always-correct fallback.
pub fn buffer_age_unsupported_on(backend: &str, renderer_name: &str) -> bool {
    backend == "winit" || buffer_age_unsupported(renderer_name)
}

/// Logs the real GL driver once, so "gles" is never mistaken for hardware acceleration.
#[allow(unsafe_code)] // glGetString is a read-only query on the current context.
pub fn log_gl_renderer(renderer: &mut GlowRenderer, backend: &str) {
    use smithay::reexports::glow::HasContext;
    let name = renderer
        .with_context(|gl| unsafe { gl.get_parameter_string(smithay::reexports::glow::RENDERER) })
        .unwrap_or_else(|_| "unknown".to_owned());
    let class = if is_software_renderer(&name) {
        "software"
    } else {
        "hardware"
    };
    if buffer_age_unsupported_on(backend, &name) {
        BUFFER_AGE_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    eprintln!("FLAMEWM_GL_RENDERER backend={backend} renderer=\"{name}\" class={class}");
}
