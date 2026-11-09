//! Child process that owns the GL device for the GPU backend. Launched by the render path, never
//! by a user; exits when the parent closes its stdin.

fn main() {
    if let Err(error) = flamewm_render_x11::serve_gpu_helper() {
        eprintln!("flamewm-gpu-helper: {error}");
        std::process::exit(1);
    }
}
