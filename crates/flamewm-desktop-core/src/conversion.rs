//! Desktop image conversion policy. Decoder/encoder delegates to the optional
//! system `ImageMagick` installation; no shell, no unbounded in-memory decoding.
//! File creation is atomic in the source directory and NEVER overwrites files.
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::file_actions::rename_entry_no_replace;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static ACTIVE_CONVERSIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_CONCURRENT_CONVERSIONS: usize = 2;
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(90);
/// Audio and video transcodes take minutes for long sources; the stalled-child
/// guard still stops a hung ffmpeg, and the timeout kills its process group.
const MEDIA_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A bounded provider slot prevents repeated menu clicks from starting an
/// unlimited number of `ImageMagick` decoders. Every return path releases it.
struct ConversionSlot;

impl ConversionSlot {
    fn acquire() -> Result<Self, String> {
        ACTIVE_CONVERSIONS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_CONCURRENT_CONVERSIONS).then_some(active + 1)
            })
            .map_err(|_| "image conversion is busy; try again shortly".to_owned())?;
        Ok(Self)
    }
}

impl Drop for ConversionSlot {
    fn drop(&mut self) {
        ACTIVE_CONVERSIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Resolve an executable without a shell. An existing, non-executable file
/// named `magick` must not shadow a working `convert` installation.
fn available_converter() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for program in ["magick", "convert"] {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(program);
            if candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111 != 0))
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn available_program(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&paths) {
        let candidate = directory.join(program);
        if candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return Some(candidate);
        }
    }
    None
}

fn wait_for_converter(command: Command) -> Result<ExitStatus, String> {
    wait_for_converter_with_timeout(command, CONVERSION_TIMEOUT)
}

/// Bumped by [`cancel_conversions`]; every job that started before a bump is killed.
static CANCEL_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Cancels every conversion or tool that is running now. Jobs started afterwards
/// are unaffected. Returns how many jobs were holding a slot when cancel was requested.
pub fn cancel_conversions() -> usize {
    CANCEL_EPOCH.fetch_add(1, Ordering::AcqRel);
    ACTIVE_CONVERSIONS.load(Ordering::Acquire)
}

/// Number of conversions or tools currently running.
#[must_use]
pub fn active_conversions() -> usize {
    ACTIVE_CONVERSIONS.load(Ordering::Acquire)
}

fn wait_for_converter_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    let mut child = command
        .spawn()
        .map_err(|err| format!("run ImageMagick: {err}"))?;
    let started = Instant::now();
    let epoch = CANCEL_EPOCH.load(Ordering::Acquire);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if CANCEL_EPOCH.load(Ordering::Acquire) != epoch => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("conversion cancelled".to_owned());
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("image conversion timed out".to_owned());
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait for ImageMagick: {err}"));
            }
        }
    }
}

/// Temporary converter files are never desktop icons; only the final atomic
/// rename produces a visible entry.
#[must_use]
pub fn is_temporary_convert_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|text| text.starts_with(".flamewm-convert-"))
}

pub const FORMAT_CHOICES: [&str; 13] = [
    "png", "webp", "jpg", "gif", "bmp", "tiff", "ico", "avif", "heic", "pdf", "txt", "md", "docx",
];
const SOURCE_EXTENSIONS: &[&str] = &[
    "png", "webp", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "ico", "avif", "heic", "heif",
];

#[must_use]
pub fn image_source(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            SOURCE_EXTENSIONS
                .iter()
                .any(|known| value.eq_ignore_ascii_case(known))
        })
}

#[must_use]
pub fn offered_formats(path: &Path) -> Vec<&'static str> {
    if image_source(path) {
        return FORMAT_CHOICES[..9]
            .iter()
            .copied()
            .filter(|choice| {
                !path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case(choice)
                            || (*choice == "jpg" && ext.eq_ignore_ascii_case("jpeg"))
                            || (*choice == "tiff" && ext.eq_ignore_ascii_case("tif"))
                    })
            })
            .collect();
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if available_program("ffmpeg").is_some() {
        let media: Vec<&'static str> = media_targets(&extension)
            .into_iter()
            .filter(|target| codecs_available(target, media_codec_args(target)))
            .collect();
        if !media.is_empty() {
            return media;
        }
    }
    let has_pandoc = available_program("pandoc").is_some();
    let has_pdftotext = available_program("pdftotext").is_some();
    match extension.as_str() {
        "pdf" if has_pdftotext => vec!["txt"],
        "md" | "markdown" if has_pandoc => vec!["txt", "pdf", "docx"],
        "txt" => {
            let mut choices = vec!["md"];
            if has_pandoc {
                choices.push("pdf");
            }
            choices
        }
        "docx" if has_pandoc => vec!["txt", "md", "pdf"],
        _ => Vec::new(),
    }
}

/// `ImageMagick` writes numbered sibling files when a multi-frame input is
/// converted to a single-frame container (e.g. GIF -> JPG or PNG). Select only
/// frame zero for those outputs; otherwise the reserved temporary remains
/// empty and invisible numbered files accumulate in the desktop directory.
/// GIF, WebP and TIFF can preserve multiple frames in one output file.
fn source_argument(source: &Path, format: &str) -> OsString {
    let mut path = source.as_os_str().to_os_string();
    if !matches!(format, "gif" | "webp" | "tiff") {
        path.push("[0]");
    }
    path
}

fn destination(source: &Path, format: &str, attempt: u32) -> Result<PathBuf, String> {
    let parent = source.parent().ok_or("source has no parent directory")?;
    let stem = source
        .file_stem()
        .ok_or("source has no file stem")?
        .to_string_lossy();
    let name = if attempt == 0 {
        format!("{stem}.{format}")
    } else {
        format!("{stem} (converted {}).{format}", attempt + 1)
    };
    Ok(parent.join(name))
}

/// The UI may show a non-authoritative prediction in this location until
/// the worker publishes. A concurrent writer can still claim it first; the
/// successful result path returned by `convert_file` remains authoritative.
pub fn predicted_destination(source: &Path, format: &str) -> Result<PathBuf, String> {
    if !offered_formats(source).contains(&format) {
        return Err("target format not offered for this file".to_owned());
    }
    for attempt in 0..1000 {
        let target = destination(source, format, attempt)?;
        if fs::symlink_metadata(&target).is_err() {
            return Ok(target);
        }
    }
    Err("no available converted filename".to_owned())
}

/// Convert beside the original. Caller MUST execute this off the UI thread.
/// Images are written into an exclusive temporary path and atomically renamed.
/// Existing destination names (including symlinks) are never replaced.
pub fn convert_image(source: &Path, format: &str) -> Result<PathBuf, String> {
    if !offered_formats(source).contains(&format) {
        return Err(format!(
            "unsupported conversion: {} -> {format}",
            source.display()
        ));
    }
    if !fs::symlink_metadata(source)
        .map_err(|e| format!("source metadata: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("conversion requires a regular file, not a symlink or directory".into());
    }
    let executable =
        available_converter().ok_or("ImageMagick missing: install the 'imagemagick' package")?;
    let _slot = ConversionSlot::acquire()?;
    let directory = source.parent().ok_or("source has no parent directory")?;
    let mut reserved = None;
    for _ in 0..64 {
        let seq = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".flamewm-convert-{}-{seq}.{format}", std::process::id());
        let temp = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(_) => {
                reserved = Some(temp);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("create temporary conversion file: {err}")),
        }
    }
    let temp = reserved.ok_or("no unique temporary conversion path")?;
    let result = (|| {
        let mut cmd = Command::new(executable);
        cmd.args([
            "-limit", "memory", "256MiB", "-limit", "map", "512MiB", "-limit", "disk", "1GiB",
        ]);
        cmd.arg(source_argument(source, format));
        if format == "jpg" || format == "bmp" {
            cmd.args(["-background", "white", "-alpha", "remove", "-alpha", "off"]);
        }
        // Explicit encoder prefix disambiguates temporary and final filenames.
        // Preserve non-UTF-8 Linux filenames: Path::display() would replace
        // invalid bytes and write to a different path.
        let mut output = OsString::from(format!("{format}:"));
        output.push(temp.as_os_str());
        cmd.arg(output);
        let status = wait_for_converter(cmd)?;
        if !status.success() {
            return Err(format!("ImageMagick {format} conversion failed: {status}"));
        }
        if fs::metadata(&temp)
            .map_err(|err| format!("conversion output: {err}"))?
            .len()
            == 0
        {
            return Err("converter produced an empty image".into());
        }
        for attempt in 0..1000 {
            let dest = destination(source, format, attempt)?;
            if fs::symlink_metadata(&dest).is_ok() {
                continue;
            }
            match rename_entry_no_replace(&temp, &dest) {
                Ok(()) => return Ok(dest),
                Err(err) => {
                    if fs::symlink_metadata(&dest).is_ok() {
                        continue;
                    }
                    return Err(format!("publish conversion: {err:?}"));
                }
            }
        }
        Err("too many files with this converted name".into())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

const AUDIO_SOURCES: [&str; 7] = ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff"];
const AUDIO_TARGETS: [&str; 8] = ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff", "wma"];
const VIDEO_SOURCES: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "wmv"];
const VIDEO_TARGETS: [&str; 8] = ["mp4", "mov", "mkv", "webm", "avi", "wmv", "gif", "mp3"];
/// Subtitle cues convert between SRT and WebVTT; both are plain text.
const SUBTITLE_FORMATS: [&str; 2] = ["srt", "vtt"];

fn media_source(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    AUDIO_SOURCES.contains(&extension.as_str())
        || VIDEO_SOURCES.contains(&extension.as_str())
        || SUBTITLE_FORMATS.contains(&extension.as_str())
}

/// Targets for an audio or video source, never the source's own format. The
/// lists fit the eight wheel slots. GIF inputs stay on the image provider.
fn media_targets(extension: &str) -> Vec<&'static str> {
    let targets: &[&'static str] = if AUDIO_SOURCES.contains(&extension) {
        &AUDIO_TARGETS
    } else if VIDEO_SOURCES.contains(&extension) {
        &VIDEO_TARGETS
    } else if SUBTITLE_FORMATS.contains(&extension) {
        &SUBTITLE_FORMATS
    } else {
        return Vec::new();
    };
    targets
        .iter()
        .copied()
        .filter(|target| *target != extension)
        .collect()
}

/// ffmpeg codec arguments per target. Audio targets drop video streams; the
/// output muxer comes from the temporary file's extension.
fn media_codec_args(format: &str) -> &'static [&'static str] {
    match format {
        "mp3" => &["-vn", "-c:a", "libmp3lame", "-q:a", "2"],
        "m4a" => &["-vn", "-c:a", "aac", "-b:a", "192k"],
        "wav" => &["-vn", "-c:a", "pcm_s16le"],
        "flac" => &["-vn", "-c:a", "flac"],
        "ogg" => &["-vn", "-c:a", "libvorbis", "-q:a", "5"],
        "opus" => &["-vn", "-c:a", "libopus", "-b:a", "128k"],
        "aiff" => &["-vn", "-c:a", "pcm_s16be"],
        "wma" => &["-vn", "-c:a", "wmav2"],
        "mp4" | "mov" | "mkv" => &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"],
        "webm" => &[
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "0",
            "-crf",
            "34",
            "-c:a",
            "libopus",
        ],
        "avi" => &["-c:v", "mpeg4", "-q:v", "4", "-c:a", "libmp3lame"],
        "wmv" => &["-c:v", "wmv2", "-c:a", "wmav2"],
        "gif" => &["-an", "-vf", "fps=12,scale=480:-2:flags=lanczos"],
        // Subtitle muxers are picked from the extension; cues need no codec.
        "srt" | "vtt" => &[],
        _ => &[],
    }
}

/// Reserve an exclusive, empty temporary output beside the source.
fn reserve_temporary(directory: &Path, format: &str) -> Result<PathBuf, String> {
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = directory.join(format!(
            ".flamewm-convert-{}-{sequence}.{format}",
            std::process::id()
        ));
        match OpenOptions::new().create_new(true).write(true).open(&temp) {
            Ok(_) => return Ok(temp),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("create conversion temporary output: {err}")),
        }
    }
    Err("no unique conversion temporary output".to_owned())
}

/// Publish a finished temporary beside the source under the first free name.
fn publish_output(temp: &Path, source: &Path, format: &str) -> Result<PathBuf, String> {
    for attempt in 0..1000 {
        let dest = destination(source, format, attempt)?;
        if fs::symlink_metadata(&dest).is_ok() {
            continue;
        }
        match rename_entry_no_replace(temp, &dest) {
            Ok(()) => return Ok(dest),
            Err(_) if fs::symlink_metadata(&dest).is_ok() => {}
            Err(err) => return Err(format!("publish converted media: {err:?}")),
        }
    }
    Err("too many files with this converted name".to_owned())
}

/// Audio and video conversion through the optional ffmpeg provider. Missing
/// ffmpeg removes these offers rather than failing at click time. Caller
/// MUST run this off the UI thread.
fn convert_media(source: &Path, format: &str) -> Result<PathBuf, String> {
    if !offered_formats(source).contains(&format) {
        return Err(format!(
            "unsupported media conversion: {} -> {format}",
            source.display()
        ));
    }
    if !fs::symlink_metadata(source)
        .map_err(|e| format!("source metadata: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("conversion source must be a regular file".to_owned());
    }
    let executable = available_program("ffmpeg")
        .ok_or("missing converter ffmpeg; install the 'ffmpeg' package")?;
    let _slot = ConversionSlot::acquire()?;
    let directory = source.parent().ok_or("conversion source has no parent")?;
    let temp = reserve_temporary(directory, format)?;
    let result = (|| {
        let mut command = Command::new(executable);
        command
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            .args(media_codec_args(format))
            .arg(&temp);
        let status = wait_for_converter_with_timeout(command, MEDIA_TIMEOUT)?;
        if !status.success() {
            return Err(format!("ffmpeg {format} conversion failed: {status}"));
        }
        if fs::metadata(&temp)
            .map_err(|err| format!("conversion output: {err}"))?
            .len()
            == 0
        {
            return Err("ffmpeg produced an empty file".to_owned());
        }
        publish_output(&temp, source, format)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// File conversion entry point. Image handling remains on the established
/// bounded `ImageMagick` provider; document formats use bounded external providers.
/// The caller must run this off the UI thread.
pub fn convert_file(source: &Path, format: &str) -> Result<PathBuf, String> {
    if image_source(source) {
        return convert_image(source, format);
    }
    if media_source(source) {
        return convert_media(source, format);
    }
    if !offered_formats(source).contains(&format) {
        return Err(format!(
            "unsupported document conversion: {} -> {format}",
            source.display()
        ));
    }
    if !fs::symlink_metadata(source)
        .map_err(|e| format!("source metadata: {e}"))?
        .file_type()
        .is_file()
    {
        return Err("conversion source must be a regular file".to_owned());
    }
    let _slot = ConversionSlot::acquire()?;
    let source_ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let provider = if source_ext == "pdf" {
        "pdftotext"
    } else {
        "pandoc"
    };
    let executable = if source_ext == "txt" && format == "md" {
        None
    } else {
        Some(available_program(provider).ok_or_else(|| {
            format!("missing converter {provider}; install pandoc / poppler-utils")
        })?)
    };
    let directory = source.parent().ok_or("conversion source has no parent")?;
    let mut reserved = None;
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = directory.join(format!(
            ".flamewm-convert-{}-{sequence}.{format}",
            std::process::id()
        ));
        match OpenOptions::new().create_new(true).write(true).open(&temp) {
            Ok(_) => {
                reserved = Some(temp);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("create conversion temporary output: {err}")),
        }
    }
    let temp = reserved.ok_or("no unique conversion temporary output")?;
    let result = (|| {
        if source_ext == "txt" && format == "md" {
            fs::copy(source, &temp).map_err(|error| format!("copy text to Markdown: {error}"))?;
        } else {
            let mut command =
                Command::new(executable.as_ref().ok_or("missing converter executable")?);
            if source_ext == "pdf" {
                // pdftotext writes UTF-8 text directly to its final argument.
                command.arg("-enc").arg("UTF-8").arg(source).arg(&temp);
            } else {
                let input = if source_ext == "docx" {
                    "docx"
                } else {
                    "markdown"
                };
                command.arg("--from").arg(input).arg("--output").arg(&temp);
                if format != "pdf" {
                    let output = match format {
                        "txt" => "plain",
                        "md" => "markdown",
                        "docx" => "docx",
                        _ => format,
                    };
                    command.arg("--to").arg(output);
                }
                command.arg(source);
            }
            let status = wait_for_converter(command)?;
            if !status.success() {
                return Err(format!(
                    "{provider} {format} conversion failed ({status}); check document dependencies"
                ));
            }
        }
        // Empty text is valid for an empty source; PDF/DOCX cannot be empty.
        if format != "txt"
            && format != "md"
            && fs::metadata(&temp)
                .map_err(|err| format!("conversion output: {err}"))?
                .len()
                == 0
        {
            return Err("converter returned an empty document".to_owned());
        }
        for attempt in 0..1000 {
            let dest = destination(source, format, attempt)?;
            if fs::symlink_metadata(&dest).is_ok() {
                continue;
            }
            match rename_entry_no_replace(&temp, &dest) {
                Ok(()) => return Ok(dest),
                Err(_) if fs::symlink_metadata(&dest).is_ok() => {}
                Err(err) => return Err(format!("publish converted document: {err:?}")),
            }
        }
        Err("too many files with this converted name".to_owned())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Encoder names ffmpeg reports, probed once. `None` when ffmpeg cannot be queried.
static FFMPEG_ENCODERS: std::sync::OnceLock<Option<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn ffmpeg_encoders() -> Option<&'static std::collections::HashSet<String>> {
    FFMPEG_ENCODERS
        .get_or_init(|| {
            let executable = available_program("ffmpeg")?;
            let output = Command::new(executable)
                .args(["-hide_banner", "-encoders"])
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            Some(
                text.lines()
                    .filter_map(|line| line.split_whitespace().nth(1))
                    .map(str::to_owned)
                    .collect(),
            )
        })
        .as_ref()
}

/// True when every encoder named by `arguments` (plus the output muxer
/// `format` when it needs one) is present. `copy` names no encoder.
fn codecs_available(format: &str, arguments: &[&str]) -> bool {
    let Some(encoders) = ffmpeg_encoders() else {
        return false;
    };
    let mut needed: Vec<&str> = arguments
        .windows(2)
        .filter(|pair| pair[0] == "-c:a" || pair[0] == "-c:v")
        .map(|pair| pair[1])
        .filter(|name| *name != "copy")
        .collect();
    if matches!(format, "gif" | "png") {
        needed.push(format);
    }
    needed.iter().all(|name| encoders.contains(*name))
}

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
fn tool_destination(
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
fn extract_provider(path: &Path) -> Option<&'static str> {
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
fn extract_archive(source: &Path, directory: &Path) -> Result<PathBuf, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    /// Tests that take conversion slots share one global budget; serialise them.
    pub(super) static SLOT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[test]
    fn same_format_is_not_offered_and_source_detection_is_case_insensitive() {
        let formats = offered_formats(Path::new("A.PNG"));
        assert!(!formats.contains(&"png"));
        assert!(formats.contains(&"webp"));
        assert!(offered_formats(Path::new("folder")).is_empty());
        assert!(!offered_formats(Path::new("readme.txt")).contains(&"webp"));
        assert!(!offered_formats(Path::new("photo.JPEG")).contains(&"jpg"));
        assert!(is_temporary_convert_name(std::ffi::OsStr::new(
            ".flamewm-convert-123-42.webp"
        )));
        assert!(!is_temporary_convert_name(std::ffi::OsStr::new(
            ".hidden-photo.webp"
        )));
    }
    #[test]
    fn output_names_remain_beside_source_without_overwriting_original() {
        let source = Path::new("/home/user/Desktop/photo.png");
        assert_eq!(
            destination(source, "webp", 0).unwrap(),
            Path::new("/home/user/Desktop/photo.webp")
        );
        assert_eq!(
            destination(source, "webp", 1).unwrap(),
            Path::new("/home/user/Desktop/photo (converted 2).webp")
        );
    }
    #[test]
    fn multiframe_sources_select_first_frame_for_single_image_destinations() {
        let source = Path::new("/home/user/Desktop/animated picture.gif");
        assert_eq!(
            source_argument(source, "jpg"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "png"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "avif"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "webp"),
            source.as_os_str().to_os_string()
        );
        assert_eq!(
            source_argument(source, "gif"),
            source.as_os_str().to_os_string()
        );
    }
    #[test]
    fn concurrent_conversion_budget_is_released_even_on_early_failure() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let first = ConversionSlot::acquire().expect("first slot");
        let second = ConversionSlot::acquire().expect("second slot");
        assert!(
            ConversionSlot::acquire().is_err(),
            "third conversion must not start"
        );
        drop(first);
        let replacement = ConversionSlot::acquire().expect("released slot reused");
        drop(second);
        drop(replacement);
    }
    #[test]
    fn converter_timeout_stops_a_blocked_child_without_waiting_for_completion() {
        let mut sleeper = Command::new("sleep");
        sleeper.arg("2");
        let start = Instant::now();
        let error = wait_for_converter_with_timeout(sleeper, Duration::from_millis(10))
            .expect_err("hung conversion must time out");
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn rejects_symlink_and_unknown_formats_before_launching_converter() {
        assert!(convert_image(Path::new("file.png"), "ps").is_err());
        assert!(convert_image(Path::new("file.txt"), "png").is_err());
    }
}

#[cfg(test)]
mod document_format_contracts {
    use super::*;
    #[test]
    fn text_file_has_markdown_target_without_optional_provider() {
        assert!(offered_formats(Path::new("notes.txt")).contains(&"md"));
    }
    #[test]
    fn unknown_binary_never_offers_document_conversion() {
        assert!(offered_formats(Path::new("program.bin")).is_empty());
    }
}

#[cfg(test)]
mod media_format_contracts {
    use super::tests::SLOT_TESTS;
    use super::*;
    #[test]
    fn audio_and_video_targets_exclude_the_source_and_fit_eight_slots() {
        for source in AUDIO_SOURCES.iter().chain(VIDEO_SOURCES.iter()) {
            let targets = media_targets(source);
            assert!(!targets.contains(source), "{source} offers itself");
            assert!(!targets.is_empty() && targets.len() <= 8, "{source}");
        }
        assert!(media_targets("png").is_empty());
        assert_eq!(media_targets("srt"), vec!["vtt"]);
        assert_eq!(media_targets("vtt"), vec!["srt"]);
        assert!(media_source(Path::new("cues.SRT")));
        assert!(media_targets("mp3").contains(&"wav"));
        assert!(!media_targets("mp3").contains(&"mp3"));
        assert!(media_targets("mp4").contains(&"gif"));
        assert!(media_targets("mp4").contains(&"mp3"));
    }
    #[test]
    fn every_media_target_has_codec_arguments_and_audio_drops_video() {
        for target in AUDIO_TARGETS.iter().chain(VIDEO_TARGETS.iter()) {
            assert!(!media_codec_args(target).is_empty(), "{target}");
        }
        for target in AUDIO_TARGETS {
            assert!(media_codec_args(target).contains(&"-vn"), "{target}");
        }
        assert!(media_codec_args("gif").contains(&"-an"));
        assert!(media_codec_args("unknown").is_empty());
    }
    #[test]
    fn ffmpeg_converts_audio_beside_the_source_without_overwriting() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        if available_program("ffmpeg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-media-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let source = dir.join("tone.wav");
        let made = Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .arg(&source)
            .status()
            .expect("ffmpeg runs");
        assert!(made.success());
        let first = convert_file(&source, "flac").expect("first conversion");
        assert_eq!(first, dir.join("tone.flac"));
        let second = convert_file(&source, "flac").expect("second conversion");
        assert_eq!(second, dir.join("tone (converted 2).flac"));
        assert!(source.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn tool_names_and_slots_fit_the_wheel() {
        assert_eq!(
            tool_destination(Path::new("/d/photo.png"), " (1280)", "png", 0).unwrap(),
            Path::new("/d/photo (1280).png")
        );
        assert_eq!(
            tool_destination(Path::new("/d/photo.png"), " (1280)", "png", 1).unwrap(),
            Path::new("/d/photo (1280) 2.png")
        );
        for tool in [
            Tool::ImageResize,
            Tool::ImageJpeg,
            Tool::AudioCompress,
            Tool::VideoFrame,
            Tool::Zip,
            Tool::Sha256,
        ] {
            assert!(!tool.label().is_empty());
        }
        assert!(
            offered_tools(Path::new("folder")).is_empty() || available_program("zip").is_some()
        );
        assert!(!offered_tools(Path::new("notes.txt")).contains(&Tool::AudioMono));
    }
    #[test]
    fn tools_run_beside_the_source_and_keep_the_original() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("flamewm-tools-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        if available_program("sha256sum").is_some() {
            let hash = run_tool(&text, Tool::Sha256).expect("sha256 tool");
            assert_eq!(hash, dir.join("notes (sha256).txt"));
            assert!(fs::read_to_string(&hash).unwrap().starts_with("2cf24dba"));
        }
        if available_program("zip").is_some() {
            let zipped = run_tool(&text, Tool::Zip).expect("zip tool");
            assert_eq!(zipped, dir.join("notes.zip"));
            assert!(text.is_file(), "original must survive");
        }
        if available_program("ffmpeg").is_some() {
            let wav = dir.join("tone.wav");
            let made = Command::new("ffmpeg")
                .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.5")
                .arg(&wav)
                .status()
                .expect("ffmpeg runs");
            assert!(made.success());
            let mono = run_tool(&wav, Tool::AudioMono).expect("mono tool");
            assert_eq!(mono, dir.join("tone (mono).wav"));
        }
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn zip_extract_writes_a_fresh_folder_and_keeps_the_archive() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        if available_program("unzip").is_none() || available_program("zip").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-extract-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        let zipped = run_tool(&text, Tool::Zip).expect("zip tool");
        let folder = run_tool(&zipped, Tool::Extract).expect("extract tool");
        assert_eq!(folder, dir.join("notes (extracted)"));
        assert_eq!(
            fs::read_to_string(folder.join("notes.txt")).expect("extracted"),
            "hello"
        );
        assert!(zipped.is_file(), "archive must survive");
        let again = run_tool(&zipped, Tool::Extract).expect("second extract");
        assert_eq!(again, dir.join("notes (extracted) 2"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_a_running_child_and_leaves_later_jobs_alone() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut sleeper = Command::new("sleep");
        sleeper.arg("5");
        let canceller = thread::spawn(|| {
            thread::sleep(Duration::from_millis(200));
            cancel_conversions();
        });
        let started = Instant::now();
        let result = wait_for_converter_with_timeout(sleeper, Duration::from_secs(30));
        canceller.join().expect("canceller thread");
        assert_eq!(result.unwrap_err(), "conversion cancelled");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "cancel must not wait for the child"
        );
        let quick = Command::new("true");
        let later = wait_for_converter_with_timeout(quick, Duration::from_secs(5));
        assert!(later.expect("job after cancel runs").success());
    }

    #[test]
    fn ffmpeg_converts_srt_to_vtt_beside_the_source() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        if available_program("ffmpeg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-subs-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let source = dir.join("cues.srt");
        fs::write(
            &source,
            "1\n00:00:00,000 --> 00:00:01,000\nHello\n\n2\n00:00:01,000 --> 00:00:02,500\nWorld\n",
        )
        .expect("write srt");
        let vtt = convert_file(&source, "vtt").expect("srt to vtt");
        assert_eq!(vtt, dir.join("cues.vtt"));
        let text = fs::read_to_string(&vtt).expect("vtt text");
        assert!(text.starts_with("WEBVTT"), "{text}");
        assert!(text.contains("00:00.000 --> 00:01.000"), "{text}");
        assert!(text.contains("World"), "{text}");
        assert!(source.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_gz_archive_tool_writes_beside_the_source() {
        let _serial = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        if available_program("tar").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-tar-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        let archive = run_tool(&text, Tool::TarGz).expect("tar tool");
        assert_eq!(archive, dir.join("notes (archive).tar.gz"));
        let listing = Command::new("tar")
            .arg("-tzf")
            .arg(&archive)
            .output()
            .expect("tar lists");
        assert!(listing.status.success());
        assert!(String::from_utf8_lossy(&listing.stdout).contains("notes.txt"));
        assert!(text.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_routing_is_case_insensitive_and_separate_from_images() {
        assert!(media_source(Path::new("Clip.MP4")));
        assert!(media_source(Path::new("song.Opus")));
        assert!(!media_source(Path::new("photo.gif")));
        assert!(!media_source(Path::new("notes.txt")));
    }
}
