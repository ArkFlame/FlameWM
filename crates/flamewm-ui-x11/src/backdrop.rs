//! Typed pseudo-transparency facade: root geometry, capture readiness and blur for product shells.

pub use flamewm_render_x11::backdrop::WindowBackdrop;
pub use flamewm_render_x11::backdrop::{
    MAX_BLUR_RADIUS_PX, blur_radius_px, blurred_argb_words, capture_is_ready,
};
pub use flamewm_render_x11::root_geometry;
