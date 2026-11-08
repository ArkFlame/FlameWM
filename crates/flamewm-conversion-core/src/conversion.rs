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

use flamewm_desktop_core::file_actions::rename_entry_no_replace;
use flamewm_desktop_core::model::TEMP_CONVERT_PREFIX;

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
        let name = format!("{TEMP_CONVERT_PREFIX}{}-{seq}.{format}", std::process::id());
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

mod media;
mod tools;

use media::*;
pub use tools::*;
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
            "{TEMP_CONVERT_PREFIX}{}-{sequence}.{format}",
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

#[cfg(test)]
mod inline_tests;
