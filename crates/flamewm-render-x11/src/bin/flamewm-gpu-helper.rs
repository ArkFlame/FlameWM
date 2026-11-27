//! Child process that owns the GL device for the GPU backend. Launched by the render path, never
//! by a user; exits when the parent closes its stdin.

fn main() {
    // A write failure here means the parent is already gone; there is nothing to degrade to, so
    // exit quietly rather than turning a dead pipe into a hard child failure.
    if let Err(error) = flamewm_render_x11::serve_gpu_helper() {
        eprintln!("flamewm-gpu-helper: {error}");
    }
}
