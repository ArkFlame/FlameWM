//! Speech and model capability traits. Everything is off by default, so the only
//! implementations shipped here report that nothing is configured.

use std::path::PathBuf;

use crate::AssistantError;

/// Speech-to-text over 16 kHz mono PCM.
pub trait Transcriber {
    fn transcribe(&mut self, pcm16_mono_16k: &[i16]) -> Result<String, AssistantError>;
}

/// Text-to-speech output.
pub trait Speech {
    fn speak(&mut self, text: &str) -> Result<(), AssistantError>;
    fn stop(&mut self);
}

/// A downloadable model. `sha256` must be checked before the file is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelManifest {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// On-demand model storage. Only runs after explicit user opt-in.
pub trait ModelStore {
    /// Returns the verified local path, reporting `(done, total)` bytes while downloading.
    fn fetch(
        &mut self,
        manifest: &ModelManifest,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, AssistantError>;
    fn installed(&self, name: &str) -> Option<PathBuf>;
}

#[derive(Debug, Default)]
pub struct UnconfiguredTranscriber;

impl Transcriber for UnconfiguredTranscriber {
    fn transcribe(&mut self, _pcm16_mono_16k: &[i16]) -> Result<String, AssistantError> {
        Err(AssistantError::NotConfigured(
            "speech-to-text is not configured: enable it and install a speech model in assistant settings".into(),
        ))
    }
}

#[derive(Debug, Default)]
pub struct UnconfiguredSpeech;

impl Speech for UnconfiguredSpeech {
    fn speak(&mut self, _text: &str) -> Result<(), AssistantError> {
        Err(AssistantError::NotConfigured(
            "text-to-speech is not configured: enable it and install a speech model in assistant settings".into(),
        ))
    }

    fn stop(&mut self) {}
}

#[derive(Debug, Default)]
pub struct UnconfiguredModelStore;

impl ModelStore for UnconfiguredModelStore {
    fn fetch(
        &mut self,
        manifest: &ModelManifest,
        _progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, AssistantError> {
        Err(AssistantError::NotConfigured(format!(
            "model download for {} is not configured; no model was fetched",
            manifest.name
        )))
    }

    fn installed(&self, _name: &str) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_report_not_configured() {
        assert!(matches!(
            UnconfiguredTranscriber.transcribe(&[0; 4]),
            Err(AssistantError::NotConfigured(_))
        ));
        assert!(matches!(UnconfiguredSpeech.speak("hi"), Err(AssistantError::NotConfigured(_))));
        let manifest = ModelManifest {
            name: "stt".into(),
            url: "https://example.invalid/m".into(),
            sha256: String::new(),
            size_bytes: 0,
        };
        let mut progress = |_: u64, _: u64| {};
        assert!(matches!(
            UnconfiguredModelStore.fetch(&manifest, &mut progress),
            Err(AssistantError::NotConfigured(_))
        ));
        assert_eq!(UnconfiguredModelStore.installed("stt"), None);
    }
}
