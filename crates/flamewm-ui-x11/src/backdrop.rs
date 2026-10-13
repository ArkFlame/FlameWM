//! Typed pseudo-transparency facade: root geometry, capture readiness and blur for product shells.

pub use flamewm_render_x11::backdrop::{
    blur_radius_px, blurred_argb_words, capture_is_ready, MAX_BLUR_RADIUS_PX,
};
pub use flamewm_render_x11::root_geometry;
