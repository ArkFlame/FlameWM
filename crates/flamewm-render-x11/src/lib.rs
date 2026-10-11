mod xft;
mod xlib;
mod xlib_ext;
mod xrender;
mod xrender_transform;
mod xshape;
mod overlay;
mod warp;
mod xcomposite;

pub use overlay::WarpOverlay;
pub use warp::WarpRect;
pub use xcomposite::CapturedImage;

pub mod backdrop;

use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::ptr;

use flamewm_reactor::{FdAction, Reactor};
use flamewm_render_core::RuntimeDocument;
pub use flamewm_render_core::{ActionEvent, ActionPhase, ControllerEvent};

mod app;
mod config;
mod external_decoration;
mod external_drawable;
mod ffi;
mod native;
mod runtime;
mod surface_controller;

use app::X11App;
pub use config::*;
pub use external_decoration::{ExternalDecorationRenderer, apply_cursor_theme};
pub use external_drawable::{
    ExternalDrawableSession, ExternalDrawableTarget, NativeDrawableRenderer,
    external_font_pattern_order, external_pixmap_byte_estimate, external_release_order,
    external_rounded_row_inset, external_symbolic_argb, external_text_measure,
};
pub use native::target::GeometryCommit;
pub use native::{PresentationState, presenter_error};
pub use runtime::*;
pub use surface_controller::*;
