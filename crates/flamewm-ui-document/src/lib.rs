//! Backend-neutral UI document runtime: decode, layout, theme and read a compiled
//! `UiTemplate` with no windowing backend. A renderer supplies the surface.

pub mod appearance_tokens;
pub mod focus;
pub mod surface_style;
mod template;

pub use flamewm_render_core::{
    CompiledDocument, CompiledNode, InteractionState, NodeKind, Overflow, Style,
};
pub use template::{
    KeyboardCapture, RuntimeImage, UiColor, UiDocument, UiDocumentAccess, UiDocumentView,
    UiTemplate, decode_document,
};
