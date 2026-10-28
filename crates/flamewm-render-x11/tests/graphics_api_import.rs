//! Guards the `flamewm_api::graphics` export that the X11 renderer consumes (E0432 regression).
use flamewm_api::graphics::{DisplayProtocol, GraphicsProvider, choose_provider};

#[test]
fn renderer_crate_resolves_graphics_provider_api() {
    assert_eq!(
        choose_provider(DisplayProtocol::X11, None, true),
        Ok(GraphicsProvider::XRender)
    );
    assert_eq!(
        choose_provider(DisplayProtocol::X11, None, false),
        Ok(GraphicsProvider::XCore)
    );
}
