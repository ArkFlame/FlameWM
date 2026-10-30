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

/// One pinned file of a model. `sha256` and `size_bytes` must be checked before the file is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// Path relative to the model directory: plain components, no `..`, no absolute paths.
    pub path: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// A downloadable model: every file the runtime needs, all pinned to one upstream commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelManifest {
    pub name: String,
    pub files: Vec<ModelFile>,
}

impl ModelManifest {
    /// Single-file manifest; `file` is the relative path inside the model directory.
    pub fn single(name: &str, file: &str, url: &str, sha256: &str, size_bytes: u64) -> Self {
        Self {
            name: name.into(),
            files: vec![ModelFile {
                path: file.into(),
                url: url.into(),
                sha256: sha256.into(),
                size_bytes,
            }],
        }
    }

    /// Sum of the pinned file sizes; the denominator of aggregated download progress.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }
}

/// On-demand model storage. Only runs after explicit user opt-in.
pub trait ModelStore {
    /// Downloads every missing file and returns the verified model directory. Reports aggregated
    /// `(done, total)` bytes across all files while downloading.
    fn fetch(
        &mut self,
        manifest: &ModelManifest,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, AssistantError>;
    /// The model directory when every pinned file is present with its pinned size.
    fn installed(&self, name: &str) -> Option<PathBuf>;
}

const TTS_COMMIT: &str = "92741c5b07d085ff840e26fea8ce43110b87852f";

/// Pinned manifest for the TTS model `gradium/phonon-31m-en` at one commit (Apache-2.0, ungated).
/// The URL is a Hugging Face `resolve/<commit>` path, never `main`. The directory holds
/// `model.q8.gguf`, `config.json`, `tokenizer.json` and `default-voice.safetensors`.
/// Speech-to-text has no manifest: its runtime fetches and verifies its own pinned model.
pub fn tts_manifest() -> ModelManifest {
    let file = |path: &str, sha256: &str, size_bytes: u64| ModelFile {
        path: path.into(),
        url: format!("https://huggingface.co/gradium/phonon-31m-en/resolve/{TTS_COMMIT}/{path}"),
        sha256: sha256.into(),
        size_bytes,
    };
    ModelManifest {
        name: "tts-phonon-local".into(),
        files: vec![
            file("model.q8.gguf", "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae", 64_512_352),
            file("config.json", "b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85", 4_634),
            file("tokenizer.json", "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936", 277_967),
            file(
                "default-voice.safetensors",
                "dc537478eaadc1a5257fcda2e3501ab81e14c111a5d95a5838f350db2afd0f1a",
                256_136,
            ),
        ],
    }
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
        let manifest = ModelManifest::single("stt", "m.bin", "https://example.invalid/m", "", 0);
        let mut progress = |_: u64, _: u64| {};
        assert!(matches!(
            UnconfiguredModelStore.fetch(&manifest, &mut progress),
            Err(AssistantError::NotConfigured(_))
        ));
        assert_eq!(UnconfiguredModelStore.installed("stt"), None);
    }

    #[test]
    fn tts_manifest_is_pinned_to_one_commit_and_passes_check_pinned() {
        let tts = tts_manifest();
        let paths: Vec<&str> = tts.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["model.q8.gguf", "config.json", "tokenizer.json", "default-voice.safetensors"]);
        assert!(crate::download::check_pinned(&tts).is_ok());
        for file in &tts.files {
            assert!(file.url.starts_with("https://huggingface.co/"), "{}", file.url);
            assert!(!file.url.contains("/resolve/main/"), "{}", file.url);
            assert_ne!(file.sha256, UNPINNED);
            assert!(file.size_bytes > 0);
        }
        assert!(tts.files.iter().all(|f| f.url.contains("/resolve/92741c5b07d085ff840e26fea8ce43110b87852f/")));
    }

    #[test]
    fn unpinned_manifest_is_still_refused() {
        let manifest = ModelManifest::single("stt-phonon-2", "m.bin", UNPINNED, UNPINNED, 0);
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
