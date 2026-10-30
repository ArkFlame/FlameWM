//! Chat providers and the selection that is persisted between runs.

use crate::openai::Endpoint;
use crate::CancelToken;
use crate::{ark, hermes};

/// Where a chat turn goes. Both share one OpenAI-compatible implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Ark,
    HermesProxy,
}

impl Provider {
    /// Menu order: ARK AI first.
    pub const ALL: [Provider; 2] = [Provider::Ark, Provider::HermesProxy];

    /// Stored in `state.json`.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Ark => "ark",
            Provider::HermesProxy => "hermes",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == id)
    }

    /// Menu header and footer name.
    pub fn title(self) -> &'static str {
        match self {
            Provider::Ark => ark::LABEL,
            Provider::HermesProxy => "Hermes",
        }
    }
}

/// The provider and model the user picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub provider: Provider,
    pub model: String,
}

impl Selection {
    pub fn ark_default() -> Self {
        Self { provider: Provider::Ark, model: ark::DEFAULT_MODEL.to_owned() }
    }

    /// Reads the stored selection. A missing provider is a legacy ARK-only state.
    pub fn from_state(provider: Option<&str>, model: Option<&str>) -> Self {
        let provider = provider.and_then(Provider::from_id).unwrap_or(Provider::Ark);
        let model = model.filter(|m| !m.is_empty()).unwrap_or(ark::DEFAULT_MODEL).to_owned();
        Self { provider, model }
    }
}

/// The selection a turn really uses. A Hermes choice falls back to ARK flash when the proxy is
/// gone, and the returned notice tells the user why.
pub fn resolve(selected: &Selection, proxy_up: bool) -> (Selection, Option<String>) {
    if selected.provider == Provider::HermesProxy && !proxy_up {
        let notice = "Hermes proxy is not running. Answered with ARK AI.".to_owned();
        return (Selection::ark_default(), Some(notice));
    }
    (selected.clone(), None)
}

/// Endpoint for a provider. `ark_key` is used only for ARK.
pub fn endpoint(provider: Provider, ark_key: &str) -> Endpoint {
    match provider {
        Provider::Ark => ark::endpoint(ark_key),
        Provider::HermesProxy => hermes::endpoint(&hermes::base_url()),
    }
}

/// Model ids of the running Hermes proxy, or `None` when it is not running.
pub fn proxy_models(cancel: &CancelToken) -> Option<Vec<String>> {
    hermes::probe(&hermes::base_url(), cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_unknown_is_none() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_id(provider.id()), Some(provider));
        }
        assert_eq!(Provider::from_id("nous"), None);
    }

    #[test]
    fn legacy_state_without_provider_maps_to_ark_flash() {
        assert_eq!(Selection::from_state(None, None), Selection::ark_default());
        assert_eq!(
            Selection::from_state(None, Some("pro")),
            Selection { provider: Provider::Ark, model: "pro".into() }
        );
        assert_eq!(
            Selection::from_state(Some("hermes"), Some("c/d")),
            Selection { provider: Provider::HermesProxy, model: "c/d".into() }
        );
    }

    #[test]
    fn hermes_turn_falls_back_to_ark_with_notice_when_proxy_is_gone() {
        let hermes = Selection { provider: Provider::HermesProxy, model: "c/d".into() };
        let (used, notice) = resolve(&hermes, false);
        assert_eq!(used, Selection::ark_default());
        assert!(notice.unwrap().contains("not running"));
    }

    #[test]
    fn running_proxy_and_ark_turns_are_unchanged() {
        let hermes = Selection { provider: Provider::HermesProxy, model: "c/d".into() };
        assert_eq!(resolve(&hermes, true), (hermes.clone(), None));
        let ark = Selection::ark_default();
        assert_eq!(resolve(&ark, false), (ark.clone(), None));
    }
}
