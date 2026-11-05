//! Backend-neutral UI document runtime: decode, layout, theme and read a compiled
//! `UiTemplate` with no windowing backend. A renderer supplies the surface.

pub mod focus;
pub mod surface_style;
mod template;

pub use flamewm_render_core::Overflow;
pub use template::{
    RuntimeImage, UiColor, UiDocument, UiDocumentAccess, UiDocumentView, UiTemplate,
    decode_document,
};
