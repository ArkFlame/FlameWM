//! One-click voice setup. A private venv under `$XDG_DATA_HOME/flamewm/assistant/voice` holds the
//! CPU speech runtimes; the pinned TTS files come through the model store; the STT runtime fetches
//! and verifies its own model into a cache inside the same directory. Two wrapper scripts adapt the
//! runtimes to the stdin/stdout contract of [`SpeechConfig`]. Nothing is installed system-wide.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::download::{CurlFetcher, DiskModelStore};
use crate::process::SystemRunner;
use crate::speech::{tts_manifest, ModelStore};
use crate::{default_models_dir, AssistantError, CancelToken, ProcessRunner, SpeechConfig};

const TORCH_INDEX: &str = "https://download.pytorch.org/whl/cpu";
const TTS_MODEL: &str = "tts-phonon-local";
const READY: &str = "ready";
const VENV_HINT: &str = "Install python3-venv (Debian or Ubuntu: sudo apt install python3-venv) and retry.";
const NET_HINT: &str = "Check the network to pypi.org, files.pythonhosted.org, download.pytorch.org and huggingface.co, then retry.";

const SPEAK: &str = r#"#!/bin/sh
# FlameWM voice wrapper. Text on stdin, WAV on stdout. $1 is the TTS model directory.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "$here/tmp.XXXXXX")
trap 'rm -rf "$work"' EXIT
"$here/../venv/bin/ptts" -m "$1" -l en -d cpu -o "$work/out.wav" -- "$(cat)" >/dev/null
cat "$work/out.wav"
"#;

const TRANSCRIBE: &str = r#"#!/bin/sh
# FlameWM voice wrapper. WAV on stdin, transcript on stdout.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "$here/tmp.XXXXXX")
trap 'rm -rf "$work"' EXIT
cat > "$work/in.wav"
FERMION_CACHE_DIR="$here/../stt-cache" "$here/../venv/bin/phonon-2" transcribe "$work/in.wav"
"#;

/// `assistant/voice` beside the models directory. `None` without a usable home or data directory.
pub fn default_voice_dir() -> Option<PathBuf> {
    default_models_dir()?.parent().map(|assistant| assistant.join("voice"))
}

/// Runs the whole setup with the real runners. `progress` receives `(percent, label)`.
pub fn setup_default(progress: &mut dyn FnMut(u8, &str)) -> Result<PathBuf, AssistantError> {
    let dir = default_voice_dir()
        .ok_or_else(|| AssistantError::Config("no data directory is available for voice files".into()))?;
    let cancel = CancelToken::new();
    let mut store = DiskModelStore::new(dir.join("models"), CurlFetcher::default()).with_cancel(cancel.clone());
    setup(&dir, &mut SystemRunner, &mut store, &cancel, progress)
}

/// Creates or repairs the voice directory and returns the TTS model directory. The ready marker is
/// written last, so an interrupted or failed setup is never reported as ready.
pub fn setup(
    dir: &Path,
    runner: &mut dyn ProcessRunner,
    store: &mut dyn ModelStore,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u8, &str),
) -> Result<PathBuf, AssistantError> {
    let text = path_arg(dir);
    if text.chars().any(char::is_whitespace) {
        return Err(AssistantError::Config(format!("the voice directory path must not contain spaces: {text}")));
    }
    let _ = fs::remove_file(dir.join(READY));
    fs::create_dir_all(dir)?;

    let venv = dir.join("venv");
    let pip = path_arg(&venv.join("bin/pip"));
    progress(5, "Creating the voice environment");
    if !venv.join("bin/pip").is_file() {
        // A venv without pip is left over from a failed run; rebuild it.
        let _ = fs::remove_dir_all(&venv);
        let venv_arg = path_arg(&venv);
        let made = exec(runner, cancel, &["python3", "-m", "venv", venv_arg.as_str()]);
        step("creating the voice environment", VENV_HINT, made)?;
    }
    progress(20, "Installing PyTorch (CPU)");
    let torch = exec(runner, cancel, &[pip.as_str(), "install", "--quiet", "--index-url", TORCH_INDEX, "torch"]);
    step("installing PyTorch", NET_HINT, torch)?;
    progress(40, "Installing the speech runtimes");
    let runtimes = exec(runner, cancel, &[pip.as_str(), "install", "--quiet", "ptts", "fermion-research"]);
    step("installing the speech runtimes", NET_HINT, runtimes)?;

    let manifest = tts_manifest();
    let total = manifest.total_bytes().max(1);
    let fetched = store.fetch(&manifest, &mut |done, _| {
        let percent = 60 + (done.min(total) * 20 / total) as u8;
        progress(percent, "Downloading the voice model");
    });
    step("downloading the voice model", NET_HINT, fetched.map(drop))?;

    progress(85, "Preparing the speech recognition model");
    let cache = format!("FERMION_CACHE_DIR={}", path_arg(&dir.join("stt-cache")));
    let phonon = path_arg(&venv.join("bin/phonon-2"));
    let stt = exec(runner, cancel, &["env", cache.as_str(), phonon.as_str(), "transcribe", "--download-only", "phonon-2"]);
    step("preparing the speech recognition model", NET_HINT, stt)?;

    progress(95, "Writing the voice commands");
    let bin = dir.join("bin");
    fs::create_dir_all(&bin)?;
    write_script(&bin.join("speak"), SPEAK)?;
    write_script(&bin.join("transcribe"), TRANSCRIBE)?;
    fs::write(dir.join(READY), "1\n")?;
    progress(100, "Voice ready");
    Ok(dir.join("models").join(TTS_MODEL))
}

/// The TTS model directory once setup has finished, or `None`.
pub fn ready_model(dir: &Path) -> Option<PathBuf> {
    let ready = dir.join(READY).is_file() && dir.join("bin/speak").is_file() && dir.join("bin/transcribe").is_file();
    ready.then(|| dir.join("models").join(TTS_MODEL))
}

/// Fills unset command templates with the wrappers from a finished setup. Commands the user set in
/// the config file are kept. Does nothing until setup has finished.
pub fn apply_defaults(cfg: &mut SpeechConfig, dir: &Path) {
    if ready_model(dir).is_none() {
        return;
    }
    let bin = dir.join("bin");
    if cfg.transcribe.is_none() {
        cfg.transcribe = Some(path_arg(&bin.join("transcribe")));
    }
    if cfg.synthesize.is_none() {
        cfg.synthesize = Some(format!("{} {{model}}", path_arg(&bin.join("speak"))));
    }
}

fn exec(runner: &mut dyn ProcessRunner, cancel: &CancelToken, argv: &[&str]) -> Result<(), AssistantError> {
    let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    runner.run(&argv, &[], cancel).map(drop)
}

/// Names the failed step and its fix. Cancellation passes through unchanged.
fn step(what: &str, hint: &str, result: Result<(), AssistantError>) -> Result<(), AssistantError> {
    match result {
        Err(AssistantError::Cancelled) => Err(AssistantError::Cancelled),
        Err(error) => Err(AssistantError::Io(format!("{what} failed: {error}. {hint}"))),
        Ok(()) => Ok(()),
    }
}

fn write_script(path: &Path, body: &str) -> Result<(), AssistantError> {
    fs::write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::ModelManifest;

    #[derive(Default)]
    struct FakeRunner {
        calls: Vec<String>,
        fail_on: Option<&'static str>,
    }

    impl ProcessRunner for FakeRunner {
        fn run(&mut self, argv: &[String], _stdin: &[u8], _cancel: &CancelToken) -> Result<Vec<u8>, AssistantError> {
            let line = argv.join(" ");
            self.calls.push(line.clone());
            match self.fail_on {
                Some(needle) if line.contains(needle) => Err(AssistantError::Io(format!("{needle} exited 1"))),
                _ => Ok(Vec::new()),
            }
        }
    }

    #[derive(Default)]
    struct FakeStore {
        fetches: usize,
        fail: bool,
    }

    impl ModelStore for FakeStore {
        fn fetch(
            &mut self,
            manifest: &ModelManifest,
            progress: &mut dyn FnMut(u64, u64),
        ) -> Result<PathBuf, AssistantError> {
            if self.fail {
                return Err(AssistantError::Unreachable("huggingface.co".into()));
            }
            self.fetches += 1;
            progress(manifest.total_bytes() / 2, manifest.total_bytes());
            Ok(PathBuf::from(&manifest.name))
        }

        fn installed(&self, _name: &str) -> Option<PathBuf> {
            None
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-voice-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn setup_runs_each_step_in_order_and_marks_ready() {
        let dir = scratch("order");
        let mut runner = FakeRunner::default();
        let mut store = FakeStore::default();
        let mut progress = Vec::new();
        let model = setup(&dir, &mut runner, &mut store, &CancelToken::new(), &mut |p, _| progress.push(p))
            .expect("setup");

        assert_eq!(model, dir.join("models/tts-phonon-local"));
        assert_eq!(runner.calls.len(), 4, "{:?}", runner.calls);
        assert!(runner.calls[0].starts_with("python3 -m venv "), "{}", runner.calls[0]);
        assert!(runner.calls[1].ends_with("--index-url https://download.pytorch.org/whl/cpu torch"));
        assert!(runner.calls[2].ends_with("install --quiet ptts fermion-research"));
        assert!(runner.calls[3].starts_with("env FERMION_CACHE_DIR="));
        assert!(runner.calls[3].ends_with("phonon-2 transcribe --download-only phonon-2"));
        assert_eq!(store.fetches, 1);
        assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
        assert_eq!(progress.last(), Some(&100));
        assert!(ready_model(&dir).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_runtime_install_names_the_step_and_stays_not_ready() {
        let dir = scratch("fail");
        let mut runner = FakeRunner { fail_on: Some("ptts"), ..FakeRunner::default() };
        let mut store = FakeStore::default();
        let error = setup(&dir, &mut runner, &mut store, &CancelToken::new(), &mut |_, _| {})
            .expect_err("pip failure");

        let AssistantError::Io(message) = error else { panic!("expected Io, got {error:?}") };
        assert!(message.starts_with("installing the speech runtimes failed:"), "{message}");
        assert!(message.contains("ptts exited 1. Check the network"), "{message}");
        assert!(message.contains("pypi.org"), "{message}");
        assert_eq!(store.fetches, 0, "no model download after a failed install");
        assert!(ready_model(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_failure_is_reported_and_not_ready() {
        let dir = scratch("model");
        let mut runner = FakeRunner::default();
        let mut store = FakeStore { fail: true, ..FakeStore::default() };
        let error = setup(&dir, &mut runner, &mut store, &CancelToken::new(), &mut |_, _| {})
            .expect_err("download failure");
        assert!(matches!(&error, AssistantError::Io(m) if m.starts_with("downloading the voice model failed")), "{error:?}");
        assert!(ready_model(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_is_not_rewritten_as_a_step_failure() {
        let result = step("installing PyTorch", NET_HINT, Err(AssistantError::Cancelled));
        assert_eq!(result, Err(AssistantError::Cancelled));
    }

    #[test]
    fn rejects_a_voice_directory_with_spaces_before_running_anything() {
        let dir = scratch("with space").join("a b");
        let mut runner = FakeRunner::default();
        let mut store = FakeStore::default();
        let error = setup(&dir, &mut runner, &mut store, &CancelToken::new(), &mut |_, _| {}).expect_err("spaces");
        assert!(matches!(error, AssistantError::Config(_)));
        assert!(runner.calls.is_empty());
    }

    #[test]
    fn apply_defaults_fills_only_unset_commands_after_setup() {
        let dir = scratch("defaults");
        setup(&dir, &mut FakeRunner::default(), &mut FakeStore::default(), &CancelToken::new(), &mut |_, _| {})
            .expect("setup");

        let mut cfg = SpeechConfig::default();
        apply_defaults(&mut cfg, &dir);
        assert_eq!(cfg.transcribe.as_deref(), Some(path_arg(&dir.join("bin/transcribe")).as_str()));
        assert_eq!(cfg.synthesize.as_deref(), Some(format!("{} {{model}}", path_arg(&dir.join("bin/speak"))).as_str()));

        let mut custom = SpeechConfig { transcribe: Some("my-stt".into()), ..SpeechConfig::default() };
        apply_defaults(&mut custom, &dir);
        assert_eq!(custom.transcribe.as_deref(), Some("my-stt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_defaults_is_a_no_op_before_setup() {
        let dir = scratch("unready");
        let mut cfg = SpeechConfig::default();
        apply_defaults(&mut cfg, &dir);
        assert_eq!(cfg, SpeechConfig::default());
    }

    #[test]
    fn wrappers_call_the_venv_runtimes_with_the_stdin_stdout_contract() {
        let dir = scratch("wrappers");
        setup(&dir, &mut FakeRunner::default(), &mut FakeStore::default(), &CancelToken::new(), &mut |_, _| {})
            .expect("setup");
        let speak = fs::read_to_string(dir.join("bin/speak")).expect("speak");
        let transcribe = fs::read_to_string(dir.join("bin/transcribe")).expect("transcribe");
        assert!(speak.contains("venv/bin/ptts") && speak.contains("-o \"$work/out.wav\"") && speak.contains("cat \"$work/out.wav\""));
        assert!(transcribe.contains("venv/bin/phonon-2") && transcribe.contains("FERMION_CACHE_DIR="));
        let mode = fs::metadata(dir.join("bin/speak")).expect("meta").permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Real install, real model downloads and real synthesis and transcription through the wrappers.
    /// Run with `FLAMEWM_VOICE_LIVE_DIR=<scratch dir without spaces>` and `--ignored`.
    #[test]
    #[ignore = "live: installs runtimes and downloads models; set FLAMEWM_VOICE_LIVE_DIR"]
    fn live_setup_round_trips_a_phrase_through_the_wrappers() {
        let Some(dir) = std::env::var_os("FLAMEWM_VOICE_LIVE_DIR").map(PathBuf::from) else { return };
        let cancel = CancelToken::new();
        let mut store = DiskModelStore::new(dir.join("models"), CurlFetcher::default());
        let model = setup(&dir, &mut SystemRunner, &mut store, &cancel, &mut |p, label| eprintln!("{p}% {label}"))
            .expect("setup");

        let mut cfg = SpeechConfig::default();
        apply_defaults(&mut cfg, &dir);
        let model_arg = model.to_string_lossy().into_owned();
        let mut runner = SystemRunner;
        let synth = crate::expand(cfg.synthesize.as_deref().expect("synth"), &[("model", &model_arg)]).expect("expand");
        let wav = runner.run(&synth, b"Voice setup works on this machine.", &cancel).expect("synthesize");
        assert!(wav.starts_with(b"RIFF") && wav.len() > 1000, "not a WAV: {} bytes", wav.len());

        let stt = crate::expand(cfg.transcribe.as_deref().expect("stt"), &[("model", &model_arg)]).expect("expand");
        let text = runner.run(&stt, &wav, &cancel).expect("transcribe");
        let text = String::from_utf8_lossy(&text).to_lowercase();
        assert!(text.contains("voice setup works"), "transcript: {text}");
    }
}
