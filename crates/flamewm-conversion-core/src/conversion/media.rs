//! Audio and video conversion through bounded ffmpeg calls.

use super::*;

pub(super) const AUDIO_SOURCES: [&str; 7] = ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff"];
pub(super) const AUDIO_TARGETS: [&str; 8] =
    ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff", "wma"];
pub(super) const VIDEO_SOURCES: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "wmv"];
pub(super) const VIDEO_TARGETS: [&str; 8] =
    ["mp4", "mov", "mkv", "webm", "avi", "wmv", "gif", "mp3"];
/// Subtitle cues convert between SRT and `WebVTT`; both are plain text.
pub(super) const SUBTITLE_FORMATS: [&str; 2] = ["srt", "vtt"];

pub(super) fn media_source(path: &Path) -> bool {
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
pub(super) fn media_targets(extension: &str) -> Vec<&'static str> {
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
pub(super) fn media_codec_args(format: &str) -> &'static [&'static str] {
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
pub(super) fn reserve_temporary(directory: &Path, format: &str) -> Result<PathBuf, String> {
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = directory.join(format!(
            "{TEMP_CONVERT_PREFIX}{}-{sequence}.{format}",
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
pub(super) fn publish_output(temp: &Path, source: &Path, format: &str) -> Result<PathBuf, String> {
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
pub(super) fn convert_media(source: &Path, format: &str) -> Result<PathBuf, String> {
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

/// Encoder names ffmpeg reports, probed once. `None` when ffmpeg cannot be queried.
pub(super) static FFMPEG_ENCODERS: std::sync::OnceLock<Option<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

pub(super) fn ffmpeg_encoders() -> Option<&'static std::collections::HashSet<String>> {
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
pub(super) fn codecs_available(format: &str, arguments: &[&str]) -> bool {
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
