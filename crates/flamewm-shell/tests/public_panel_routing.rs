//! Compile contract: the shell binary is an external consumer of the shell library.
//! In Rust, `src/main.rs` cannot call `pub(crate)` methods defined in `src/lib.rs`.

use flamewm_shell::{ShellRuntime, ShellSurfaces};

#[test]
fn shell_binary_panel_routing_methods_are_public() {
    let _set_origin = ShellRuntime::select_panel_output;
    let _route_surface = ShellSurfaces::select_input_output;
}
