use super::{
    buffer_age_probe_allowed, buffer_age_unsupported, frame_age, is_software_renderer,
};

#[test]
fn buffer_age_is_not_queried_before_first_submit() {
    assert!(!buffer_age_probe_allowed(false, false));
    assert!(buffer_age_probe_allowed(true, false));
    assert!(!buffer_age_probe_allowed(true, true));
}

#[test]
fn mesa_cpu_rasterizers_are_software() {
    assert!(is_software_renderer("llvmpipe (LLVM 19.1.0, 256 bits)"));
    assert!(is_software_renderer("Software Rasterizer"));
    assert!(!is_software_renderer(
        "Mesa Intel(R) UHD Graphics (TGL GT1)"
    ));
}

#[test]
fn software_renderer_skips_buffer_age_probe() {
    // The nested llvmpipe surface answers EGL_BAD_SURFACE to BUFFER_AGE (one ERROR per start).
    assert!(buffer_age_unsupported("llvmpipe (LLVM 21.1.8, 256 bits)"));
    assert!(!buffer_age_unsupported(
        "Mesa Intel(R) UHD Graphics (TGL GT1)"
    ));
}

#[test]
fn nested_winit_skips_buffer_age_probe_on_hardware_too() {
    // Real Intel GL on the nested winit surface: EGL_BAD_SURFACE on every age query.
    assert!(super::buffer_age_unsupported_on(
        "winit",
        "Mesa Intel(R) UHD Graphics (TGL GT1)"
    ));
    assert!(!super::buffer_age_unsupported_on(
        "drm",
        "Mesa Intel(R) UHD Graphics (TGL GT1)"
    ));
    assert!(super::buffer_age_unsupported_on(
        "drm",
        "llvmpipe (LLVM 21.1.8, 256 bits)"
    ));
}
