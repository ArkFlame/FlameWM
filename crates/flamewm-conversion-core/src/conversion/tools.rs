//! Quick file tools (resize, archive extraction) and their dispatch.

use super::*;

/// A quick file tool. Each writes a new file beside the source; the original
/// is never changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    ImageResize,
    ImageJpeg,
    ImageStrip,
    ImageRotate,
    ImageGrayscale,
    AudioCompress,
    AudioNormalize,
    AudioFirst30,
    AudioMono,
    VideoCompress,
    VideoStrip,
    VideoMute,
    VideoFirst10,
    VideoFrame,
    Zip,
    Sha256,
    TarGz,
    Extract,
}

impl Tool {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ImageResize => "Resize to 1280",
            Self::ImageJpeg => "Compress to JPEG",
            Self::ImageStrip | Self::VideoStrip => "Strip metadata",
            Self::ImageRotate => "Rotate 90",
            Self::ImageGrayscale => "Grayscale",
            Self::AudioCompress | Self::VideoCompress => "Compress",
            Self::AudioNormalize => "Normalize",
            Self::AudioFirst30 => "First 30 seconds",
            Self::AudioMono => "Mono",
            Self::VideoMute => "Mute",
            Self::VideoFirst10 => "First 10 seconds",
            Self::VideoFrame => "Save frame",
            Self::Zip => "ZIP",
            Self::Sha256 => "SHA-256",
            Self::TarGz => "tar.gz",
            Self::Extract => "Extract",
        }
    }

    /// Name suffix placed before the extension; the original name is kept.
    const fn suffix(self) -> &'static str {
        match self {
            Self::ImageResize => " (1280)",
            Self::ImageJpeg | Self::VideoCompress | Self::AudioCompress => " (compressed)",
            Self::ImageStrip | Self::VideoStrip => " (clean)",
            Self::ImageRotate => " (rotated)",
            Self::ImageGrayscale => " (gray)",
            Self::AudioNormalize => " (normalized)",
            Self::AudioFirst30 => " (30s)",
            Self::AudioMono => " (mono)",
            Self::VideoMute => " (muted)",
            Self::VideoFirst10 => " (10s)",
            Self::VideoFrame => " (frame)",
            Self::Zip => "",
            Self::Sha256 => " (sha256)",
            Self::TarGz => " (archive)",
            Self::Extract => " (extracted)",
        }
    }

    /// Output extension; `None` keeps the source's extension.
    const fn output(self) -> Option<&'static str> {
        match self {
            Self::ImageJpeg | Self::AudioCompress => Some("jpg"),
            Self::VideoCompress => Some("mp4"),
            Self::VideoFrame => Some("png"),
            Self::Zip => Some("zip"),
            Self::Sha256 => Some("txt"),
            Self::TarGz => Some("tar.gz"),
            _ => None,
        }
    }

    /// Provider program (`magick` is resolved like the image converter).
    const fn provider(self) -> &'static str {
        match self {
            Self::ImageResize
            | Self::ImageJpeg
            | Self::ImageStrip
            | Self::ImageRotate
            | Self::ImageGrayscale => "magick",
            Self::Zip => "zip",
            Self::Sha256 => "sha256sum",
            Self::TarGz => "tar",
            Self::Extract => "archive",
            _ => "ffmpeg",
        }
    }

    /// Provider arguments between the input and the output path.
    const fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::ImageResize => &["-resize", "1280x1280>"],
            Self::ImageJpeg => &[
                "-background",
                "white",
                "-alpha",
                "remove",
                "-alpha",
                "off",
                "-quality",
                "80",
            ],
            Self::ImageStrip | Self::VideoStrip => &["-strip"],
            Self::ImageRotate => &["-rotate", "90"],
            Self::ImageGrayscale => &["-colorspace", "Gray"],
            Self::AudioCompress => &["-vn", "-c:a", "libmp3lame", "-b:a", "128k"],
            Self::AudioNormalize => &["-vn", "-af", "loudnorm"],
            Self::AudioFirst30 => &["-t", "30"],
            Self::AudioMono => &["-ac", "1"],
            Self::VideoCompress => &[
                "-c:v", "libx264", "-crf", "28", "-preset", "veryfast", "-pix_fmt", "yuv420p",
                "-c:a", "aac", "-b:a", "128k",
            ],
            Self::VideoMute => &["-an", "-c:v", "copy"],
            Self::VideoFirst10 => &["-t", "10"],
            Self::VideoFrame => &["-ss", "1", "-frames:v", "1"],
            Self::Zip | Self::Sha256 | Self::TarGz | Self::Extract => &[],
        }
    }
}

/// Tools for one file, filtered by the providers and encoders present. The
/// audio/video list keeps the menu order; ZIP and SHA-256 apply to
/// every regular file.
#[must_use]
pub fn offered_tools(path: &Path) -> Vec<Tool> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut tools = Vec::new();
    if image_source(path) && extension != "gif" {
        if available_converter().is_some() {
            tools.extend([
                Tool::ImageResize,
                Tool::ImageJpeg,
                Tool::ImageStrip,
                Tool::ImageRotate,
                Tool::ImageGrayscale,
            ]);
        }
    } else if AUDIO_SOURCES.contains(&extension.as_str()) {
        tools.extend([
            Tool::AudioCompress,
            Tool::AudioNormalize,
            Tool::AudioFirst30,
            Tool::AudioMono,
        ]);
    } else if VIDEO_SOURCES.contains(&extension.as_str()) || extension == "gif" {
        tools.extend([
            Tool::VideoCompress,
            Tool::VideoStrip,
            Tool::VideoMute,
            Tool::VideoFirst10,
            Tool::VideoFrame,
        ]);
    }
    tools.retain(|tool| {
        tool.provider() != "ffmpeg"
            || (available_program("ffmpeg").is_some()
                && codecs_available(tool.output().unwrap_or(""), tool.arguments()))
    });
    if available_program("zip").is_some() {
        tools.push(Tool::Zip);
    }
    if available_program("sha256sum").is_some() {
        tools.push(Tool::Sha256);
    }
    if available_program("tar").is_some() {
        tools.push(Tool::TarGz);
    }
    if extract_provider(path).is_some_and(|program| available_program(program).is_some()) {
        tools.push(Tool::Extract);
    }
    tools.truncate(8);
    tools
}

/// Output name for a tool: `stem<suffix>.ext`, then numbered on collision.
pub(super) fn tool_destination(
    source: &Path,
    suffix: &str,
    extension: &str,
    attempt: u32,
) -> Result<PathBuf, String> {
    let parent = source.parent().ok_or("source has no parent directory")?;
    let stem = source
        .file_stem()
        .ok_or("source has no file stem")?
        .to_string_lossy();
    let name = if attempt == 0 {
        format!("{stem}{suffix}.{extension}")
    } else {
        format!("{stem}{suffix} {}.{extension}", attempt + 1)
    };
    Ok(parent.join(name))
}

/// Archive provider for the Extract tool, from the file name. `None` for other files.
pub(super) fn extract_provider(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some("unzip")
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some("tar")
    } else {
        None
    }
}

/// Extracts a zip or tar.gz into a new folder beside it. The folder is created
/// exclusively (never reused) and unzip runs with `-n`, so no existing file is replaced.
/// The archive is untrusted: `tar` runs without same-owner and stdin is closed.
pub(super) fn extract_archive(source: &Path, directory: &Path) -> Result<PathBuf, String> {
    let provider = extract_provider(source).ok_or("not a supported archive")?;
    let program = available_program(provider)
        .ok_or_else(|| format!("missing '{provider}': install it to extract archives"))?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("archive has no file name")?;
    let suffix_len = if name.to_ascii_lowercase().ends_with(".tar.gz") {
        7
    } else {
        4
    };
    let base = &name[..name.len() - suffix_len];
    let mut destination = None;
    for attempt in 0..100u32 {
        let candidate = if attempt == 0 {
            directory.join(format!("{base} (extracted)"))
        } else {
            directory.join(format!("{base} (extracted) {}", attempt + 1))
        };
        match fs::create_dir(&candidate) {
            Ok(()) => {
                destination = Some(candidate);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("create extract folder: {err}")),
        }
    }
    let destination = destination.ok_or("too many extracted folders with this name")?;
    let mut command = Command::new(program);
    command.stdin(std::process::Stdio::null());
    if provider == "unzip" {
        command
            .args(["-q", "-n"])
            .arg(source)
            .arg("-d")
            .arg(&destination);
    } else {
        command
            .args(["-xzf"])
            .arg(source)
            .arg("--no-same-owner")
            .arg("-C")
            .arg(&destination);
    }
    let outcome = wait_for_converter_with_timeout(command, MEDIA_TIMEOUT).and_then(|status| {
        if status.success() {
            Ok(())
        } else {
            Err(format!("{provider} extract failed: {status}"))
        }
    });
    match outcome {
        Ok(()) => Ok(destination),
        Err(error) => {
            let _ = fs::remove_dir_all(&destination);
            Err(error)
        }
    }
}

/// Run one quick tool off the UI thread. Same reservation, timeout and
/// no-overwrite publishing rules as conversions.
pub fn run_tool(source: &Path, tool: Tool) -> Result<PathBuf, String> {
    if !offered_tools(source).contains(&tool) {
        return Err(format!("tool not offered for this file: {}", tool.label()));
    }
    if !fs::symlink_metadata(source)
        .map_err(|e| format!("source metadata: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("tool source must be a regular file".to_owned());
    }
    let source_extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let extension = tool.output().unwrap_or(&source_extension).to_owned();
    let directory = source.parent().ok_or("tool source has no parent")?;
    let _slot = ConversionSlot::acquire()?;
    if tool == Tool::Extract {
        return extract_archive(source, directory);
    }
    let temp = reserve_temporary(directory, &extension)?;
    let result = (|| {
        let timeout = if tool.provider() == "ffmpeg" {
            MEDIA_TIMEOUT
        } else {
            CONVERSION_TIMEOUT
        };
        match tool.provider() {
            "sha256sum" => {
                let output =
                    fs::File::create(&temp).map_err(|err| format!("tool output: {err}"))?;
                let name = source.file_name().ok_or("source has no file name")?;
                let status = wait_for_converter_with_timeout(
                    {
                        let mut command = Command::new("sha256sum");
                        command.current_dir(directory).arg(name).stdout(output);
                        command
                    },
                    timeout,
                )?;
                if !status.success() {
                    return Err(format!("sha256sum failed: {status}"));
                }
            }
            "tar" => {
                // tar writes into the reserved empty file; the source name is relative to its folder.
                let name = source.file_name().ok_or("source has no file name")?;
                let mut command = Command::new("tar");
                command
                    .current_dir(directory)
                    .arg("-czf")
                    .arg(&temp)
                    .arg("--")
                    .arg(name);
                let status = wait_for_converter_with_timeout(command, timeout)?;
                if !status.success() {
                    return Err(format!("tar failed: {status}"));
                }
            }
            "zip" => {
                // zip rejects a zero-byte file as a broken archive. Drop our own
                // reservation so zip creates the archive at the reserved name.
                fs::remove_file(&temp).map_err(|err| format!("reserve zip output: {err}"))?;
                let mut command = Command::new("zip");
                command.arg("-q").arg("-j").arg(&temp).arg(source);
                let status = wait_for_converter_with_timeout(command, timeout)?;
                if !status.success() {
                    return Err(format!("zip failed: {status}"));
                }
            }
            "ffmpeg" => {
                let mut command =
                    Command::new(available_program("ffmpeg").ok_or("missing ffmpeg")?);
                command
                    .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(source)
                    .args(tool.arguments())
                    .arg(&temp);
                let status = wait_for_converter_with_timeout(command, timeout)?;
                if !status.success() {
                    return Err(format!("ffmpeg {} failed: {status}", tool.label()));
                }
            }
            _ => {
                let executable = available_converter()
                    .ok_or("ImageMagick missing: install the 'imagemagick' package")?;
                let mut command = Command::new(executable);
                command.args([
                    "-limit", "memory", "256MiB", "-limit", "map", "512MiB", "-limit", "disk",
                    "1GiB",
                ]);
                command.arg(source_argument(source, &extension));
                command.args(tool.arguments());
                let mut output = OsString::from(format!("{extension}:"));
                output.push(temp.as_os_str());
                command.arg(output);
                let status = wait_for_converter_with_timeout(command, timeout)?;
                if !status.success() {
                    return Err(format!("ImageMagick {} failed: {status}", tool.label()));
                }
            }
        }
        if fs::metadata(&temp)
            .map_err(|err| format!("tool output: {err}"))?
            .len()
            == 0
        {
            return Err("tool produced an empty file".to_owned());
        }
        for attempt in 0..1000 {
            let dest = tool_destination(source, tool.suffix(), &extension, attempt)?;
            if fs::symlink_metadata(&dest).is_ok() {
                continue;
            }
            match rename_entry_no_replace(&temp, &dest) {
                Ok(()) => return Ok(dest),
                Err(_) if fs::symlink_metadata(&dest).is_ok() => {}
                Err(err) => return Err(format!("publish tool output: {err:?}")),
            }
        }
        Err("too many files with this tool name".to_owned())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
