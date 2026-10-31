//! External command seam for speech. Commands come from a plain config file as whitespace-split
//! templates (no shell). Every process is cancellable and killed on cancel.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::{AssistantError, CancelToken};

/// Runs an argv with bytes on stdin and returns stdout. Implementations must return
/// `AssistantError::Cancelled` promptly once `cancel` is set, and must not run a shell.
pub trait ProcessRunner: Send {
    fn run(
        &mut self,
        argv: &[String],
        stdin: &[u8],
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, AssistantError>;
}

/// Substitutes `{name}` placeholders token by token. Errors on an empty template or an
/// unknown placeholder, so a typo fails loudly instead of running with a literal `{model}`.
pub fn expand(template: &str, vars: &[(&str, &str)]) -> Result<Vec<String>, AssistantError> {
    let argv: Vec<String> = template
        .split_whitespace()
        .map(|token| {
            let mut out = token.to_owned();
            for (name, value) in vars {
                out = out.replace(&format!("{{{name}}}"), value);
            }
            out
        })
        .collect();
    if argv.is_empty() {
        return Err(AssistantError::NotConfigured(
            "external command template is empty".into(),
        ));
    }
    if let Some(token) = argv.iter().find(|t| t.contains('{') && t.contains('}')) {
        return Err(AssistantError::Config(format!(
            "unknown placeholder in command: {token}"
        )));
    }
    Ok(argv)
}

/// Runs real processes. Missing programs map to `NotConfigured` so the UI can explain them.
#[derive(Debug, Default)]
pub struct SystemRunner;

impl ProcessRunner for SystemRunner {
    fn run(
        &mut self,
        argv: &[String],
        stdin: &[u8],
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, AssistantError> {
        let program = argv
            .first()
            .ok_or_else(|| AssistantError::NotConfigured("empty command".into()))?;
        if cancel.is_cancelled() {
            return Err(AssistantError::Cancelled);
        }
        let mut child = spawn(argv)?;
        if let Some(mut pipe) = child.stdin.take() {
            // The writer is detached: a child that exits early or is killed breaks the pipe.
            let payload = stdin.to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&payload);
            });
        }
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let stdout = thread::spawn(move || read_all(out.take()));
        let stderr = thread::spawn(move || read_all(err.take()));
        let status = wait_or_cancel(&mut child, cancel)?;
        let bytes = stdout.join().unwrap_or_default();
        let message = String::from_utf8_lossy(&stderr.join().unwrap_or_default())
            .trim()
            .to_owned();
        if status.success() {
            Ok(bytes)
        } else {
            let detail = if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            };
            Err(AssistantError::Io(format!(
                "{program} exited with {status}{detail}"
            )))
        }
    }
}

fn spawn(argv: &[String]) -> Result<Child, AssistantError> {
    let program = &argv[0];
    Command::new(program)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AssistantError::NotConfigured(format!(
                "{program} was not found; it is an optional external command"
            )),
            _ => AssistantError::Io(format!("{program}: {error}")),
        })
}

fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    buf
}

fn wait_or_cancel(
    child: &mut Child,
    cancel: &CancelToken,
) -> Result<std::process::ExitStatus, AssistantError> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AssistantError::Cancelled);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// A microphone capture in progress. Raw signed 16-bit little-endian PCM from the command's stdout.
pub struct Recording {
    child: Child,
    reader: JoinHandle<Vec<u8>>,
}

/// Starts a capture command. Stop it with [`Recording::finish`].
pub fn start_recording(argv: &[String]) -> Result<Recording, AssistantError> {
    let mut child = spawn(argv)?;
    drop(child.stdin.take());
    let mut out = child.stdout.take();
    let reader = thread::spawn(move || read_all(out.take()));
    Ok(Recording { child, reader })
}

impl Recording {
    /// Stops capture and returns the samples recorded so far.
    #[must_use]
    pub fn finish(mut self) -> Vec<i16> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let bytes = self.reader.join().unwrap_or_default();
        bytes
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    }
}

/// Command templates from `$XDG_CONFIG_HOME/flamewm/assistant-speech.conf`.
///
/// Format: `key = command` per line, `#` starts a comment. Keys: `record_command`
/// (default `arecord -q -t raw -f S16_LE -r 16000 -c 1`), `transcribe_command` (`{model}`),
/// `synthesize_command` (`{model}`, text on stdin, WAV on stdout), `play_command` (`{file}`).
/// Transcribe and synthesize have no default because no speech runtime is verified here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechConfig {
    pub record: String,
    pub transcribe: Option<String>,
    pub synthesize: Option<String>,
    pub play: String,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            record: "arecord -q -t raw -f S16_LE -r 16000 -c 1".into(),
            transcribe: None,
            synthesize: None,
            play: "aplay -q {file}".into(),
        }
    }
}

impl SpeechConfig {
    /// Parses config text. Unknown keys are an error so typos do not silently disable a command.
    pub fn parse(text: &str) -> Result<Self, AssistantError> {
        let mut config = Self::default();
        for (number, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| {
                AssistantError::Config(format!("line {}: expected key = command", number + 1))
            })?;
            let value = value.trim().to_owned();
            match key.trim() {
                "record_command" => config.record = value,
                "transcribe_command" => config.transcribe = Some(value),
                "synthesize_command" => config.synthesize = Some(value),
                "play_command" => config.play = value,
                other => return Err(AssistantError::Config(format!("unknown key: {other}"))),
            }
        }
        Ok(config)
    }

    /// Loads the config file. A missing file means defaults.
    pub fn load(path: &Path) -> Result<Self, AssistantError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(AssistantError::Io(error.to_string())),
        }
    }
}

/// `$XDG_CONFIG_HOME/flamewm/assistant-speech.conf`, falling back to `~/.config`.
pub fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".config")))?;
    Some(base.join("flamewm").join("assistant-speech.conf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_placeholders_and_rejects_unknown_ones() {
        let argv = expand(
            "tool --model {model} --out {file}",
            &[("model", "/m"), ("file", "/f.wav")],
        )
        .unwrap();
        assert_eq!(argv, ["tool", "--model", "/m", "--out", "/f.wav"]);
        assert!(matches!(
            expand("tool {modle}", &[("model", "/m")]),
            Err(AssistantError::Config(_))
        ));
        assert!(matches!(
            expand("   ", &[]),
            Err(AssistantError::NotConfigured(_))
        ));
    }

    #[test]
    fn config_defaults_and_rejects_unknown_keys() {
        let config =
            SpeechConfig::parse("# c\ntranscribe_command = stt --model {model}\n").unwrap();
        assert_eq!(config.transcribe.as_deref(), Some("stt --model {model}"));
        assert_eq!(config.synthesize, None);
        assert_eq!(config.record, SpeechConfig::default().record);
        assert!(matches!(
            SpeechConfig::parse("speak = x"),
            Err(AssistantError::Config(_))
        ));
        assert!(matches!(
            SpeechConfig::parse("no equals"),
            Err(AssistantError::Config(_))
        ));
    }

    #[test]
    fn cancel_kills_a_running_process_promptly() {
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        let argv = vec!["sleep".to_owned(), "30".to_owned()];
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        let result = SystemRunner.run(&argv, b"", &cancel);
        canceller.join().unwrap();
        assert_eq!(result, Err(AssistantError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_program_is_not_configured() {
        let argv = vec!["flamewm-no-such-command-xyz".to_owned()];
        let result = SystemRunner.run(&argv, b"", &CancelToken::new());
        assert!(matches!(result, Err(AssistantError::NotConfigured(_))));
    }

    #[test]
    fn recording_returns_samples_written_before_finish() {
        // printf emits two little-endian samples (1 and -2), then the process idles until killed.
        let argv = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            r"printf '\001\000\376\377'; sleep 30".to_owned(),
        ];
        let recording = start_recording(&argv).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(recording.finish(), vec![1, -2]);
    }
}
