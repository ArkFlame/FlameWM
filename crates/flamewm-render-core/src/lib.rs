pub mod codec;
pub mod hash;
pub mod layout;
pub mod model;
pub mod paint;
pub mod pixels;
pub mod surface_material;
pub mod text_layout;

pub use codec::{decode, decode_static, encode};
pub use layout::{
    IntrinsicMeasureError, IntrinsicSize, LayoutBox, LayoutEngine, LayoutResult,
    layout_passes_on_this_thread,
};
pub use model::*;
pub use pixels::{PixelBackend, PixelOp, RenderAcceleration, RoundedRect, composite_cpu};
pub use surface_material::{
    SurfaceMaterial, gloss_line, rim_alpha_at, shadow_ring_alpha, shadow_rings,
};
pub use paint::{
    PaintCommand, build_paint_commands, build_paint_commands_with_scroll, emit_scroll_chrome,
    scroll_clip, scrollbar_visible,
};
pub use text_layout::{TextLayout, TextLine, layout_text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionPhase {
    Press,
    Hover,
    Motion,
    Release,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionEvent {
    pub action: String,
    pub phase: ActionPhase,
    pub button: u32,
    /// Raw X11 modifier mask captured on the event (Shift=bit 0, Control=bit 2).
    /// This is event-time state, not a keyboard-state guess from a prior frame.
    pub modifiers: u32,
    pub x: f32,
    pub y: f32,
    pub inside: bool,
    pub time_ms: u64,
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControllerEvent {
    Action(ActionEvent),
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
