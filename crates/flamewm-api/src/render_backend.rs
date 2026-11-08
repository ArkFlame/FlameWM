//! Render backend vocabulary shared by both sessions and the settings surface.
//!
//! `renderBackend` is the request. `renderAcceleration` (`cpu`/`gpu`) is the older key and
//! still applies when `renderBackend` is unset or unreadable. Resolution is pure: the caller
//! applies the effective backend and logs [`RenderBackendChoice::reason`] when a request was
//! not honoured.

use crate::settings::{SettingValue, SettingsSnapshot};

pub const RENDER_BACKEND_SETTING: &str = "renderBackend";
pub const RENDER_ACCELERATION_SETTING: &str = "renderAcceleration";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderBackend {
    /// Software rendering. The product default.
    #[default]
    Cpu,
    /// Pick the best available backend. Not probed yet, so it resolves to CPU.
    Auto,
    /// EGL/GLES acceleration for blur and batched paints.
    OpenGl,
    /// Accepted for forward compatibility. This build has no Vulkan backend.
    Vulkan,
}

impl RenderBackend {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "cpu" => Some(Self::Cpu),
            "opengl" => Some(Self::OpenGl),
            "vulkan" => Some(Self::Vulkan),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::OpenGl => "opengl",
            Self::Vulkan => "vulkan",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderBackendChoice {
    /// Backend to run, after every fallback in this build.
    pub backend: RenderBackend,
    /// One line for the log when the request was not honoured as written.
    pub reason: Option<String>,
}

/// Resolves stored settings to the backend this build runs.
#[must_use]
pub fn choose(
    render_backend: Option<&str>,
    render_acceleration: Option<&str>,
) -> RenderBackendChoice {
    let legacy = legacy_backend(render_acceleration);
    let (requested, mut reason) = match render_backend {
        None => (legacy, None),
        Some(value) => match RenderBackend::parse(value) {
            Some(backend) => (backend, None),
            None => (
                legacy,
                Some(format!(
                    "unknown renderBackend {value:?}, using renderAcceleration"
                )),
            ),
        },
    };
    let backend = match requested {
        RenderBackend::Vulkan => {
            reason = Some("vulkan is not built into this binary, using opengl".to_owned());
            RenderBackend::OpenGl
        }
        RenderBackend::Auto => {
            reason = Some("auto needs the GPU probe, which is not built yet, using cpu".to_owned());
            RenderBackend::Cpu
        }
        other => other,
    };
    RenderBackendChoice { backend, reason }
}

/// Resolves from a settings snapshot. Missing or non-text values count as unset.
#[must_use]
pub fn choose_from_snapshot(snapshot: &SettingsSnapshot) -> RenderBackendChoice {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => Some(value.as_str()),
        _ => None,
    };
    choose(
        text(RENDER_BACKEND_SETTING),
        text(RENDER_ACCELERATION_SETTING),
    )
}

fn legacy_backend(render_acceleration: Option<&str>) -> RenderBackend {
    match render_acceleration.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("gpu") => RenderBackend::OpenGl,
        _ => RenderBackend::Cpu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_each_value_case_insensitively_and_rejects_others() {
        assert_eq!(RenderBackend::parse(" Auto "), Some(RenderBackend::Auto));
        assert_eq!(RenderBackend::parse("CPU"), Some(RenderBackend::Cpu));
        assert_eq!(RenderBackend::parse("opengl"), Some(RenderBackend::OpenGl));
        assert_eq!(RenderBackend::parse("Vulkan"), Some(RenderBackend::Vulkan));
        assert_eq!(RenderBackend::parse("gpu"), None);
        assert_eq!(RenderBackend::parse(""), None);
    }

    #[test]
    fn as_setting_round_trips_through_parse() {
        for backend in [
            RenderBackend::Auto,
            RenderBackend::Cpu,
            RenderBackend::OpenGl,
            RenderBackend::Vulkan,
        ] {
            assert_eq!(RenderBackend::parse(backend.as_setting()), Some(backend));
        }
    }

    #[test]
    fn unset_settings_resolve_to_cpu_without_a_reason() {
        assert_eq!(
            choose(None, None),
            RenderBackendChoice {
                backend: RenderBackend::Cpu,
                reason: None,
            }
        );
    }

    #[test]
    fn legacy_key_migrates_gpu_to_opengl_and_cpu_to_cpu() {
        assert_eq!(choose(None, Some("gpu")).backend, RenderBackend::OpenGl);
        assert_eq!(choose(None, Some(" GPU ")).backend, RenderBackend::OpenGl);
        assert_eq!(choose(None, Some("cpu")).backend, RenderBackend::Cpu);
        assert_eq!(choose(None, Some("bogus")).backend, RenderBackend::Cpu);
        assert_eq!(choose(None, Some("gpu")).reason, None);
    }

    #[test]
    fn explicit_render_backend_wins_over_legacy_key() {
        assert_eq!(choose(Some("cpu"), Some("gpu")).backend, RenderBackend::Cpu);
        assert_eq!(
            choose(Some("opengl"), Some("cpu")).backend,
            RenderBackend::OpenGl
        );
    }

    #[test]
    fn vulkan_request_falls_back_to_opengl_with_a_reason() {
        let choice = choose(Some("vulkan"), None);
        assert_eq!(choice.backend, RenderBackend::OpenGl);
        assert!(
            choice
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("vulkan"))
        );
    }

    #[test]
    fn auto_resolves_to_cpu_with_a_reason_until_probing_exists() {
        let choice = choose(Some("auto"), Some("gpu"));
        assert_eq!(choice.backend, RenderBackend::Cpu);
        assert!(choice.reason.as_deref().is_some_and(|r| r.contains("auto")));
    }

    #[test]
    fn unknown_render_backend_uses_legacy_key_and_says_so() {
        let choice = choose(Some("metal"), Some("gpu"));
        assert_eq!(choice.backend, RenderBackend::OpenGl);
        assert!(
            choice
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("metal"))
        );
    }

    #[test]
    fn snapshot_treats_missing_and_non_text_values_as_unset() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(choose_from_snapshot(&snapshot).backend, RenderBackend::Cpu);

        snapshot.values.insert(
            RENDER_ACCELERATION_SETTING.to_owned(),
            SettingValue::Text("gpu".to_owned()),
        );
        assert_eq!(
            choose_from_snapshot(&snapshot).backend,
            RenderBackend::OpenGl
        );

        snapshot.values.insert(
            RENDER_BACKEND_SETTING.to_owned(),
            SettingValue::Boolean(true),
        );
        assert_eq!(
            choose_from_snapshot(&snapshot).backend,
            RenderBackend::OpenGl
        );

        snapshot.values.insert(
            RENDER_BACKEND_SETTING.to_owned(),
            SettingValue::Text("cpu".to_owned()),
        );
        assert_eq!(choose_from_snapshot(&snapshot).backend, RenderBackend::Cpu);
    }
}
