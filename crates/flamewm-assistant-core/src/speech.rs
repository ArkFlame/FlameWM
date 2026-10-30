//! Speech and model capability traits, plus command-backed implementations. Everything is off by
//! default; the stubs report that nothing is configured.
//!
//! STT and TTS run external commands from [`SpeechConfig`](crate::process::SpeechConfig). Audio
//! capture and playback are optional externals (`arecord`, `aplay`). Nothing here has been run
//! against a real speech model or audio device.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::process::{expand, ProcessRunner};
use crate::{AssistantError, CancelToken};

/// Manifest placeholder for values not verified offline. Never downloaded.
pub const UNPINNED: &str = "unpinned";

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

/// Pinned manifests for the two Phonon models. Each URL is a Hugging Face `resolve/<commit>` path
/// (never `main`); the sha256 is the LFS oid published for that commit and was matched against a
/// local download of each file.
///
/// - `tts-phonon-local`: `gradium/phonon-31m-en` `model.q8.gguf` (Apache-2.0, ungated). The
///   companion files `config.json`, `tokenizer.json` and `default-voice.safetensors` at the same
///   commit are NOT covered by this single-file manifest; see docs/DEPENDENCIES.md.
/// - `stt-phonon-2`: `FermionResearch/Phonon-2` `phonon-2.bps.tar.zst` (CC-BY-4.0 weights, ungated).
///   This is the packed model archive; its consumer format is not verified by this crate.
pub fn default_manifests() -> Vec<ModelManifest> {
    vec![
        ModelManifest {
            name: "tts-phonon-local".into(),
            url: "https://huggingface.co/gradium/phonon-31m-en/resolve/92741c5b07d085ff840e26fea8ce43110b87852f/model.q8.gguf".into(),
            sha256: "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae".into(),
            size_bytes: 64_512_352,
        },
        ModelManifest {
            name: "stt-phonon-2".into(),
            url: "https://huggingface.co/FermionResearch/Phonon-2/resolve/3e6ab3a81d73b4d424ff34cc72535deb606378ef/phonon-2.bps.tar.zst".into(),
            sha256: "98125795b6dda72f5c6eee9ba33d19815df65dcb18b50a357bf9f73c9935309e".into(),
            size_bytes: 163_515_201,
        },
    ]
}

/// Cancels whichever speech is currently running. Clones share state, so the UI can stop speech
/// from its own thread while the speaking thread owns the [`CommandSpeech`].
#[derive(Clone, Debug, Default)]
pub struct Stopper(Arc<Mutex<Option<CancelToken>>>);

impl Stopper {
    pub fn stop(&self) {
        if let Some(token) = self.0.lock().map(|slot| slot.clone()).unwrap_or(None) {
            token.cancel();
        }
    }
}

/// Encodes 16 kHz mono 16-bit PCM as a canonical 44-byte-header WAV file.
pub fn encode_wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Transcribes by sending a WAV file on stdin to the configured command. `{model}` is the
/// verified model path; the transcript is read from stdout.
pub struct CommandTranscriber {
    runner: Box<dyn ProcessRunner>,
    template: String,
    model: PathBuf,
    cancel: CancelToken,
}

impl CommandTranscriber {
    pub fn new(runner: Box<dyn ProcessRunner>, template: &str, model: PathBuf, cancel: CancelToken) -> Self {
        Self { runner, template: template.to_owned(), model, cancel }
    }
}

impl Transcriber for CommandTranscriber {
    fn transcribe(&mut self, pcm16_mono_16k: &[i16]) -> Result<String, AssistantError> {
        let model = self.model.to_string_lossy().into_owned();
        let argv = expand(&self.template, &[("model", &model)])?;
        let wav = encode_wav(pcm16_mono_16k);
        let stdout = self.runner.run(&argv, &wav, &self.cancel)?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
    }
}

/// Speaks by synthesizing a WAV with the configured command (text on stdin, WAV on stdout), then
/// playing it with the play command. `stop` or [`Stopper::stop`] kills whichever step is running.
pub struct CommandSpeech {
    runner: Box<dyn ProcessRunner>,
    synthesize: String,
    play: String,
    model: PathBuf,
    scratch: PathBuf,
    current: Stopper,
    counter: u64,
}

impl CommandSpeech {
    /// `scratch` receives the temporary WAV file and is created on demand.
    pub fn new(
        runner: Box<dyn ProcessRunner>,
        synthesize: &str,
        play: &str,
        model: PathBuf,
        scratch: PathBuf,
    ) -> Self {
        Self {
            runner,
            synthesize: synthesize.to_owned(),
            play: play.to_owned(),
            model,
            scratch,
            current: Stopper::default(),
            counter: 0,
        }
    }

    /// Handle for stopping speech from another thread.
    pub fn stopper(&self) -> Stopper {
        self.current.clone()
    }
}

impl Speech for CommandSpeech {
    fn speak(&mut self, text: &str) -> Result<(), AssistantError> {
        let model = self.model.to_string_lossy().into_owned();
        let synth = expand(&self.synthesize, &[("model", &model)])?;
        let token = CancelToken::new();
        if let Ok(mut slot) = self.current.0.lock() {
            *slot = Some(token.clone());
        }
        let result = self.speak_with(&synth, text, &token);
        if let Ok(mut slot) = self.current.0.lock() {
            *slot = None;
        }
        result
    }

    fn stop(&mut self) {
        self.current.stop();
    }
}

impl CommandSpeech {
    fn speak_with(&mut self, synth: &[String], text: &str, token: &CancelToken) -> Result<(), AssistantError> {
        let wav = self.runner.run(synth, text.as_bytes(), token)?;
        if token.is_cancelled() {
            return Err(AssistantError::Cancelled);
        }
        fs::create_dir_all(&self.scratch)?;
        self.counter += 1;
        let file = self.scratch.join(format!("tts-{}-{}.wav", std::process::id(), self.counter));
        fs::write(&file, &wav)?;
        let played = play_file(self.runner.as_mut(), &self.play, &file, token);
        let _ = fs::remove_file(&file);
        played
    }
}

fn play_file(
    runner: &mut dyn ProcessRunner,
    template: &str,
    file: &Path,
    token: &CancelToken,
) -> Result<(), AssistantError> {
    let file = file.to_string_lossy().into_owned();
    let argv = expand(template, &[("file", &file)])?;
    runner.run(&argv, &[], token).map(|_| ())
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
    use crate::test_support::test_dir;
    use std::sync::Mutex as StdMutex;

    /// Records every call and answers from a queue of canned stdout values.
    struct FakeRunner {
        calls: Arc<StdMutex<Vec<(Vec<String>, Vec<u8>)>>>,
        replies: Vec<Vec<u8>>,
    }

    impl ProcessRunner for FakeRunner {
        fn run(&mut self, argv: &[String], stdin: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, AssistantError> {
            if cancel.is_cancelled() {
                return Err(AssistantError::Cancelled);
            }
            self.calls.lock().unwrap().push((argv.to_vec(), stdin.to_vec()));
            Ok(if self.replies.is_empty() { Vec::new() } else { self.replies.remove(0) })
        }
    }

    #[allow(clippy::type_complexity)]
    fn fake(replies: Vec<Vec<u8>>) -> (Box<dyn ProcessRunner>, Arc<StdMutex<Vec<(Vec<String>, Vec<u8>)>>>) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        (Box::new(FakeRunner { calls: calls.clone(), replies }), calls)
    }

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

    #[test]
    fn default_manifests_are_pinned_to_commits_and_pass_check_pinned() {
        let manifests = default_manifests();
        assert_eq!(manifests.len(), 2);
        for manifest in &manifests {
            assert!(manifest.url.starts_with("https://huggingface.co/"), "{}", manifest.url);
            assert!(manifest.url.contains("/resolve/"), "{}", manifest.url);
            assert!(!manifest.url.contains("/resolve/main/"), "{}", manifest.url);
            assert_ne!(manifest.sha256, UNPINNED);
            assert!(manifest.size_bytes > 0);
            assert!(crate::download::check_pinned(manifest).is_ok(), "{}", manifest.name);
        }
    }

    #[test]
    fn unpinned_manifest_is_still_refused() {
        let manifest = ModelManifest {
            name: "stt-phonon-2".into(),
            url: UNPINNED.into(),
            sha256: UNPINNED.into(),
            size_bytes: 0,
        };
        assert!(crate::download::check_pinned(&manifest).is_err());
    }

    #[test]
    fn wav_header_matches_pcm_length() {
        let wav = encode_wav(&[1, -1, 300]);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 6);
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16_000);
    }

    #[test]
    fn transcriber_sends_wav_and_trims_transcript() {
        let (runner, calls) = fake(vec![b"  hello there \n".to_vec()]);
        let mut stt = CommandTranscriber::new(runner, "stt --model {model}", "/m/stt".into(), CancelToken::new());
        assert_eq!(stt.transcribe(&[5, 6]).unwrap(), "hello there");
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].0, ["stt", "--model", "/m/stt"]);
        assert_eq!(&calls[0].1[0..4], b"RIFF");
        assert_eq!(calls[0].1.len(), 48);
    }

    #[test]
    fn speak_synthesizes_then_plays_a_temporary_file_and_cleans_up() {
        let dir = test_dir("speech-speak");
        let (runner, calls) = fake(vec![b"RIFFfake".to_vec(), Vec::new()]);
        let mut tts = CommandSpeech::new(
            runner,
            "synth --model {model}",
            "play {file}",
            "/m/tts".into(),
            dir.clone(),
        );
        tts.speak("Hello").unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].0, ["synth", "--model", "/m/tts"]);
        assert_eq!(calls[0].1, b"Hello");
        assert_eq!(calls[1].0[0], "play");
        let played = PathBuf::from(&calls[1].0[1]);
        assert!(played.starts_with(&dir));
        assert!(!played.exists(), "temporary WAV is removed after playback");
    }

    #[test]
    fn unconfigured_synthesis_command_is_not_configured() {
        let (runner, calls) = fake(vec![]);
        let mut tts = CommandSpeech::new(runner, "   ", "play {file}", "/m".into(), test_dir("speech-empty"));
        assert!(matches!(tts.speak("x"), Err(AssistantError::NotConfigured(_))));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn stopper_cancels_the_running_speech_step() {
        struct StopOnSynth(Stopper);
        impl ProcessRunner for StopOnSynth {
            fn run(&mut self, _: &[String], _: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, AssistantError> {
                self.0.stop();
                if cancel.is_cancelled() { Err(AssistantError::Cancelled) } else { Ok(Vec::new()) }
            }
        }
        let dir = test_dir("speech-stop");
        let mut tts = CommandSpeech::new(
            Box::new(StopOnSynth(Stopper::default())),
            "synth",
            "play {file}",
            "/m".into(),
            dir,
        );
        // Re-wire the runner's stopper to this speech's current slot.
        let stopper = tts.stopper();
        tts.runner = Box::new(StopOnSynth(stopper.clone()));
        assert_eq!(tts.speak("x"), Err(AssistantError::Cancelled));
        // A new speech after a stop starts with a fresh token.
        assert!(stopper.0.lock().unwrap().is_none());
    }
}
