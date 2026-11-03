//! Backend-neutral graphics provider negotiation. No rendering implementation or native
//! handles belong here: X11 and Wayland keep their existing production render pipelines.

/// Backend-neutral name every session accepts; the protocol-specific variable wins over it.
pub const LEGACY_PROVIDER_VARIABLE: &str = "FLAMEWM_RENDER_PROVIDER";

/// X11 sessions read this first, then [`LEGACY_PROVIDER_VARIABLE`].
pub const X11_PROVIDER_VARIABLE: &str = "FLAMEWM_X11_RENDER_PROVIDER";

/// Wayland sessions read this first, then [`LEGACY_PROVIDER_VARIABLE`].
pub const WAYLAND_PROVIDER_VARIABLE: &str = "FLAMEWM_WAYLAND_RENDER_PROVIDER";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayProtocol {
    X11,
    Wayland,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsProvider {
    /// Server-side `XRender` compositing. GPU execution depends on the X server/driver.
    XRender,
    /// Standard X11 core operations when `XRender` is unavailable or deliberately disabled.
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

/// Resolves a provider request from the environment: `primary` (the protocol-specific variable)
/// first, then `legacy` (the generic `FLAMEWM_RENDER_PROVIDER` alias every backend accepts).
///
/// The value is trimmed and an empty or whitespace-only value counts as absent, so
/// `FLAMEWM_RENDER_PROVIDER=` behaves exactly like an unset variable instead of selecting a
/// provider named "". [`choose_provider`] keeps deciding which values are acceptable; this only
/// answers *what was asked for*.
#[must_use]
pub fn provider_request(primary: &str, legacy: &str) -> Option<String> {
    resolve_provider_request(primary, legacy, |name| std::env::var(name).ok())
        .map(|(value, _)| value)
}

/// The variable [`provider_request`] took its value from.
///
/// Only the rejection diagnostic needs it: a rejected request has to name the variable the user
/// actually set, because naming one that was never read would send them to the wrong place. This
/// re-reads the environment, so it belongs on a failure path, not in a paint path.
#[must_use]
pub fn provider_request_variable<'a>(primary: &'a str, legacy: &'a str) -> Option<&'a str> {
    resolve_provider_request(primary, legacy, |name| std::env::var(name).ok())
        .map(|(_, source)| source)
}

/// One rule for "which variable wins and what did it say", so the value and the diagnostic can
/// never disagree. `lookup` is injected so the rule is testable without touching the process
/// environment.
fn resolve_provider_request<'a>(
    primary: &'a str,
    legacy: &'a str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<(String, &'a str)> {
    [primary, legacy].into_iter().find_map(|name| {
        lookup(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|value| (value, name))
    })
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

    /// The protocol-specific variable wins over the generic alias, and the generic alias is read
    /// when the protocol variable says nothing. `FLAMEWM_RENDER_PROVIDER` is the documented
    /// backend-neutral name, so it must not be a no-op.
    #[test]
    fn protocol_variable_wins_over_the_generic_alias() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
            }
        };
        let resolve = |pairs: &[(&str, &str)]| {
            resolve_provider_request("PRIMARY", "LEGACY", env(pairs)).map(|(value, _)| value)
        };

        assert_eq!(
            resolve(&[("PRIMARY", "core"), ("LEGACY", "xrender")]).as_deref(),
            Some("core"),
            "the protocol variable is the specific request"
        );
        assert_eq!(
            resolve(&[("LEGACY", "xrender")]).as_deref(),
            Some("xrender"),
            "the generic alias is read when the protocol variable is unset"
        );
        assert_eq!(
            resolve(&[("PRIMARY", "  gles  ")]).as_deref(),
            Some("gles"),
            "surrounding whitespace is trimmed"
        );
        assert_eq!(
            resolve(&[("PRIMARY", "   "), ("LEGACY", "core")]).as_deref(),
            Some("core"),
            "a blank value is absent, not a request for a provider named ''"
        );
        assert_eq!(resolve(&[("PRIMARY", ""), ("LEGACY", "")]), None);
        assert_eq!(resolve(&[]), None);
    }

    /// The diagnostic names the variable that was read, so a rejected value points the user at the
    /// variable they actually set.
    #[test]
    fn the_reported_variable_is_the_one_that_was_read() {
        let resolve = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            resolve_provider_request("PRIMARY", "LEGACY", move |name| {
                pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
            })
            .map(|(_, source)| source)
        };
        assert_eq!(resolve(&[("PRIMARY", "core")]), Some("PRIMARY"));
        assert_eq!(resolve(&[("LEGACY", "core")]), Some("LEGACY"));
        assert_eq!(
            resolve(&[("PRIMARY", " "), ("LEGACY", "core")]),
            Some("LEGACY")
        );
        assert_eq!(resolve(&[]), None);
    }

    /// A value the negotiator rejects still reaches it unchanged, so the failure text can quote the
    /// user's own string.
    #[test]
    fn an_unsupported_value_reaches_the_negotiator() {
        let requested = resolve_provider_request("PRIMARY", "LEGACY", |name| {
            (name == "LEGACY").then(|| "vulkan".to_owned())
        });
        assert_eq!(requested.map(|(value, _)| value).as_deref(), Some("vulkan"));
        assert!(choose_provider(DisplayProtocol::X11, Some("vulkan"), true).is_err());
    }

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
