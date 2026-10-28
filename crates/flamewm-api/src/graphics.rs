//! Backend-neutral graphics provider negotiation. No rendering implementation or native
//! handles belong here: X11 and Wayland keep their existing production render pipelines.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayProtocol {
    X11,
    Wayland,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsProvider {
    /// Server-side XRender compositing. GPU execution depends on the X server/driver.
    XRender,
    /// Standard X11 core operations when XRender is unavailable or deliberately disabled.
    XCore,
    /// Smithay's OpenGL ES renderer. The GL driver may itself be software-rendered.
    Gles,
}

impl GraphicsProvider {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::XRender => "xrender",
            Self::XCore => "core",
            Self::Gles => "gles",
        }
    }
}

/// Select a provider that the *current* native backend can actually use. Unsupported explicit
/// requests are errors, not silent substitutions. `xrender_available` is detected after binding
/// the real X11 drawable, so the selection cannot claim acceleration from mere library presence.
pub fn choose_provider(
    protocol: DisplayProtocol,
    requested: Option<&str>,
    xrender_available: bool,
) -> Result<GraphicsProvider, String> {
    let requested = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("auto");
    match (protocol, requested) {
        (DisplayProtocol::Wayland, "auto" | "gles") => Ok(GraphicsProvider::Gles),
        (DisplayProtocol::X11, "core") => Ok(GraphicsProvider::XCore),
        (DisplayProtocol::X11, "auto" | "xrender") if xrender_available => {
            Ok(GraphicsProvider::XRender)
        }
        (DisplayProtocol::X11, "auto") => Ok(GraphicsProvider::XCore),
        (DisplayProtocol::X11, "xrender") => {
            Err("requested XRender provider is unavailable on this display".into())
        }
        _ => Err(format!(
            "unsupported graphics provider '{requested}' for {protocol:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_chooses_real_xrender_or_core_fallback() {
        assert_eq!(
            choose_provider(DisplayProtocol::X11, None, true).unwrap(),
            GraphicsProvider::XRender
        );
        assert_eq!(
            choose_provider(DisplayProtocol::X11, None, false).unwrap(),
            GraphicsProvider::XCore
        );
        assert_eq!(
            choose_provider(DisplayProtocol::X11, Some("core"), true).unwrap(),
            GraphicsProvider::XCore
        );
        assert!(choose_provider(DisplayProtocol::X11, Some("xrender"), false).is_err());
    }

    #[test]
    fn wayland_requires_actual_gles_renderer() {
        assert_eq!(
            choose_provider(DisplayProtocol::Wayland, None, false).unwrap(),
            GraphicsProvider::Gles
        );
        assert_eq!(
            choose_provider(DisplayProtocol::Wayland, Some(" gles "), false).unwrap(),
            GraphicsProvider::Gles
        );
        assert!(choose_provider(DisplayProtocol::Wayland, Some("vulkan"), false).is_err());
        assert!(choose_provider(DisplayProtocol::X11, Some("gles"), true).is_err());
    }
}
