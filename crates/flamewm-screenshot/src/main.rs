//! `FlameWM` Screenshot: an on-demand, native Rust/X11 capture, selection,
//! annotation, export and clipboard utility. No Qt/GTK/WebView process.

mod capture;
mod clipboard;
mod notify;
mod record;
mod ui;
mod wayland;

use capture::{Capture as _, Image, Rect};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use x11rb::connection::Connection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Full,
    Screen,
    Region,
    Active,
    Window,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Screen => "screen",
            Self::Region => "region",
            Self::Active => "active",
            Self::Window => "window",
        }
    }
}

#[derive(Debug)]
struct Options {
    mode: Mode,
    save: bool,
    copy: bool,
    annotate: bool,
    cursor: bool,
    output: Option<PathBuf>,
    name_template: String,
    delay: Duration,
    record: Option<u64>,
    fps: u32,
    audio_source: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Region,
            save: true,
            copy: false,
            annotate: false,
            cursor: false,
            output: None,
            name_template: "FlameWM-{timestamp}-{nanos}".into(),
            delay: Duration::ZERO,
            record: None,
            fps: 30,
            audio_source: None,
        }
    }
}

const HELP: &str = "FlameWM Screenshot — lightweight native Rust/X11 capture\n\
Usage: flamewm-screenshot [--region|--full|--screen|--active|--window] [OPTIONS]\n\
  --region         Drag to select a rectangular region (default; Esc cancels)\n\
  --full           Capture all X11 monitors as one canvas\n\
  --screen         Monitor containing the mouse pointer\n\
  --active         Active window, including FlameWM decorations\n\
  --window         Window under the pointer\n\
  --annotate       Native editor: P pen, L line, R rectangle, E ellipse,\n\
                   A arrow, X blackout, C color, U undo, Enter save, Esc cancel\n\
  --copy           Copy image/png to the X11 clipboard (survives command exit)\n\
  --no-save        Do not save to Pictures/Screenshots; combine with --copy\n\
  --output PATH    Save new PNG at PATH (never silently overwrite)\n\
  --name-template STEM  Saved filename; supports {timestamp}, {nanos}, {mode},\n\
                        {width}, and {height}; extension is added automatically\n\
  --delay SEC      Capture delay, 0..30 seconds\n\
  --cursor         Include mouse pointer in saved image (if XFixes available)\n\
  --record SEC     Record X11 screen/region/window, 1..3600 seconds;\n\
                   requires optional ffmpeg; output supports MP4, WebM or GIF\n\
  --fps N          Recording frame rate, 1..60 (default 30)\n\
  --audio-source SOURCE  Optional PulseAudio monitor source (off by default)\n\
  -h, --help       Show this help\n\
Notes: X11 only. Window capture records visible pixels, not obscured surfaces.\n";

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--region" => o.mode = Mode::Region,
            "--full" => o.mode = Mode::Full,
            "--screen" => o.mode = Mode::Screen,
            "--active" => o.mode = Mode::Active,
            "--window" => o.mode = Mode::Window,
            "--copy" | "--clipboard" => o.copy = true,
            "--no-save" => o.save = false,
            "--annotate" => o.annotate = true,
            "--cursor" => o.cursor = true,
            "--output" | "--name-template" | "--delay" | "--record" | "--fps"
            | "--audio-source" => {
                let kind = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {kind}"))?;
                match kind {
                    "--output" => {
                        o.output = Some(PathBuf::from(value));
                        o.save = true;
                    }
                    "--name-template" => {
                        if value.is_empty() {
                            return Err("name template must not be empty".into());
                        }
                        o.name_template.clone_from(value);
                    }
                    "--delay" => {
                        let seconds = value.parse::<f64>().map_err(|_| "delay must be a number")?;
                        if !seconds.is_finite() || !(0.0..=30.0).contains(&seconds) {
                            return Err("delay must be 0..30 seconds".into());
                        }
                        o.delay = Duration::from_secs_f64(seconds);
                    }
                    "--record" => {
                        let seconds = value
                            .parse::<u64>()
                            .map_err(|_| "record duration must be an integer")?;
                        if !(1..=3600).contains(&seconds) {
                            return Err("record duration must be 1..3600 seconds".into());
                        }
                        o.record = Some(seconds);
                    }
                    "--fps" => {
                        o.fps = value
                            .parse::<u32>()
                            .map_err(|_| "fps must be a positive integer")?;
                        if !(1..=60).contains(&o.fps) {
                            return Err("fps must be 1..60".into());
                        }
                    }
                    "--audio-source" => {
                        if value.is_empty() {
                            return Err("audio source must not be empty".into());
                        }
                        o.audio_source = Some(value.clone());
                    }
                    _ => unreachable!(),
                }
            }
            "--serve-clipboard" | "--temporary-clipboard" => {
                return Err("internal clipboard mode must be invoked by the owner".into());
            }
            other => return Err(format!("unknown option {other}; use --help")),
        }
        i += 1;
    }
    if !o.save && !o.copy && o.record.is_none() {
        return Err("--no-save requires --copy".into());
    }
    if o.record.is_some() {
        if o.copy || o.annotate || o.cursor {
            return Err("--record does not support --copy, --cursor or --annotate".into());
        }
        if let Some(path) = &o.output {
            match path.extension().and_then(|s| s.to_str()) {
                Some("mp4" | "webm") => {}
                Some("gif") if o.audio_source.is_none() => {}
                Some("gif") => return Err("GIF does not support audio".into()),
                _ => return Err("video output path must use .mp4, .webm or .gif".into()),
            }
        }
    } else if o.audio_source.is_some() {
        return Err("--audio-source requires --record".into());
    }
    Ok(o)
}

fn pictures_dir() -> PathBuf {
    // XDG user-dirs is intentionally not parsed as shell: unsafe interpolation must never execute.
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    let config =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    if let Ok(contents) = fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in contents.lines() {
            if let Some((_, value)) = line.trim().split_once('=') {
                if !line.trim().starts_with("XDG_PICTURES_DIR=") {
                    continue;
                }
                let value = value.trim().trim_matches('"');
                if let Some(relative) = value.strip_prefix("$HOME/") {
                    if !relative.contains("..") {
                        return home.join(relative).join("Screenshots");
                    }
                } else if value == "$HOME" {
                    return home.join("Screenshots");
                } else if Path::new(value).is_absolute() {
                    return PathBuf::from(value).join("Screenshots");
                }
            }
        }
    }
    home.join("Pictures/Screenshots")
}
fn render_name(
    template: &str,
    mode: Mode,
    rect: Rect,
    extension: &str,
    timestamp: u64,
    nanos: u128,
) -> Result<String, String> {
    let name = template
        .replace("{timestamp}", &timestamp.to_string())
        .replace("{nanos}", &format!("{nanos:09}"))
        .replace("{mode}", mode.as_str())
        .replace("{width}", &rect.w.to_string())
        .replace("{height}", &rect.h.to_string());
    if name.is_empty()
        || matches!(name.as_str(), "." | "..")
        || name.contains(['/', '\\', '\0'])
        || name.contains(['{', '}'])
    {
        return Err("name template must produce one plain filename with known placeholders".into());
    }
    Ok(format!("{name}.{extension}"))
}

fn default_output(
    extension: &str,
    template: &str,
    mode: Mode,
    rect: Rect,
) -> Result<PathBuf, String> {
    let dir = pictures_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = render_name(
        template,
        mode,
        rect,
        extension,
        flamewm_api::clock::unix_seconds(),
        flamewm_api::clock::unix_nanos() % 1_000_000_000,
    )?;
    Ok(dir.join(name))
}

fn new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    // create_new guards user data against accidental overwrite and symlink tricks.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.flush()) {
        let _ = fs::remove_file(path);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(())
}

fn selection<C: Connection>(
    conn: &C,
    screen: usize,
    mode: Mode,
    backend: &dyn capture::Capture,
) -> Result<(Rect, Option<Image>), String> {
    let root = conn.setup().roots[screen].root;
    let full = capture::root_bounds(conn, screen);
    match mode {
        Mode::Full => Ok((full, None)),
        Mode::Screen => Ok((capture::current_screen(conn, root, full)?, None)),
        Mode::Active | Mode::Window => {
            let window = if mode == Mode::Active {
                capture::active_window(conn, root)?
            } else {
                capture::pointer_window(conn, root)?
            };
            let rect = capture::window_rect(conn, root, window)?
                .intersect(full)
                .ok_or("window is offscreen")?;
            Ok((rect, None))
        }
        Mode::Region => {
            let full_image = backend.capture(full)?;
            let Some(rect) = ui::select_region(conn, screen, &full_image)? else {
                return Err("cancelled".into());
            };
            Ok((rect, Some(full_image)))
        }
    }
}

fn record_video(
    rect: Rect,
    duration: u64,
    fps: u32,
    path: &Path,
    audio_source: Option<&str>,
) -> Result<record::Stats, String> {
    if !record::ffmpeg_available() {
        return Err(
            "screen recording needs ffmpeg on PATH (not found); screenshots work without it".into(),
        );
    }
    if path.exists() {
        return Err(format!("output already exists: {}", path.display()));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let args = record::encoder_args(extension, fps, (rect.w, rect.h), audio_source)?;
    let mut child = std::process::Command::new("ffmpeg")
        .args(args)
        .arg("-n")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg unavailable: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg stdin unavailable")?;
    let stop = Arc::new(AtomicBool::new(false));
    let indicator = notify::recording_indicator(stop.clone());
    if indicator.is_none() {
        eprintln!("FlameWM: stop indicator unavailable; recording ends after {duration} s");
    }
    let recorded = record::run(
        record::x11_frames(rect),
        &mut stdin,
        fps,
        Duration::from_secs(duration),
        &stop,
    );
    // Closing stdin lets ffmpeg finish the container; a failed run leaves no partial file.
    drop(stdin);
    drop(indicator);
    let mut diagnostics = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut stderr, &mut diagnostics);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let outcome = match recorded {
        Ok(stats) if status.success() => Ok(stats),
        Ok(_) => Err(format!(
            "recording failed: ffmpeg exited {status}: {}",
            diagnostics.trim()
        )),
        Err(error) => Err(error),
    };
    if outcome.is_err() {
        let _ = fs::remove_file(path);
    }
    outcome
}

/// Encodes, saves, copies and announces one finished capture. Shared by the X11 and Wayland paths.
fn save_capture(o: Options, rect: capture::Rect, captured: capture::Image) -> Result<(), String> {
    let bytes = captured.png()?;
    let mut saved = None;
    if o.save {
        let path = o
            .output
            .map_or_else(|| default_output("png", &o.name_template, o.mode, rect), Ok)?;
        new_file(&path, &bytes)?;
        println!("{}", path.display());
        saved = Some(path);
    }
    if o.copy {
        let temporary = saved.is_none();
        let temporary_path;
        let path = if let Some(p) = saved.as_ref() {
            p.as_path()
        } else {
            temporary_path = std::env::temp_dir().join(format!(
                "flamewm-clipboard-{}-{}.png",
                std::process::id(),
                flamewm_api::clock::unix_nanos()
            ));
            new_file(&temporary_path, &bytes)?;
            &temporary_path
        };
        clipboard::spawn_owner(path, temporary)?;
        eprintln!("FlameWM: image copied to clipboard");
    }
    notify::captured(saved.as_deref(), o.copy);
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    if args.first().is_some_and(|s| s == "--serve-clipboard") {
        let path = args.get(1).ok_or("missing clipboard path")?;
        return clipboard::serve(
            Path::new(path),
            args.get(2).is_some_and(|s| s == "--temporary-clipboard"),
        );
    }
    if args.iter().any(|s| s == "--help" || s == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    let o = parse(args)?;
    // Full-output captures on a native Wayland session go through screencopy: the X11 root never
    // holds native Wayland windows. Anything else (region, window, recording) stays on X11.
    if o.record.is_none() && !o.annotate && matches!(o.mode, Mode::Full | Mode::Screen) {
        if o.delay > Duration::ZERO {
            std::thread::sleep(o.delay);
        }
        if let Some(image) = wayland::capture_output()? {
            let rect = capture::Rect {
                x: 0,
                y: 0,
                w: image.width,
                h: image.height,
            };
            return save_capture(o, rect, image);
        }
    }
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connection failed: {e}"))?;
    let backend = capture::X11Capture::new(&conn, screen);
    if o.delay > Duration::ZERO {
        std::thread::sleep(o.delay);
    }
    let (rect, full_image) = match selection(&conn, screen, o.mode, &backend) {
        Ok(found) => found,
        Err(e) if e == "cancelled" => {
            eprintln!("FlameWM: screenshot cancelled");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if let Some(duration) = o.record {
        let path = o
            .output
            .map_or_else(|| default_output("mp4", &o.name_template, o.mode, rect), Ok)?;
        let stats = record_video(rect, duration, o.fps, &path, o.audio_source.as_deref())?;
        eprintln!(
            "FlameWM: {} frames written, {} captured, {} dropped",
            stats.written, stats.captured, stats.dropped
        );
        println!("{}", path.display());
        return Ok(());
    }
    let mut captured = if let Some(full) = full_image {
        full.crop(rect)?
    } else {
        backend.capture(rect)?
    };
    if o.cursor {
        capture::blend_cursor(&conn, &mut captured, rect)?;
    }
    let captured = if o.annotate {
        let Some(annotated) = ui::annotate(&conn, screen, captured)? else {
            return Ok(());
        };
        annotated
    } else {
        captured
    };
    save_capture(o, rect, captured)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&args) {
        eprintln!("flamewm-screenshot: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_save_requires_clipboard() {
        assert!(parse(&["--no-save".into()]).is_err());
        assert!(parse(&["--no-save".into(), "--copy".into()]).is_ok());
    }
    #[test]
    fn invalid_delay_is_rejected() {
        for value in ["-1", "NaN", "100", "Infinity"] {
            assert!(parse(&["--delay".into(), value.into()]).is_err());
        }
    }
    #[test]
    fn recording_does_not_silently_create_screenshots() {
        assert!(parse(&["--record".into(), "10".into(), "--copy".into()]).is_err());
        assert_eq!(
            parse(&["--record".into(), "15".into()]).unwrap().record,
            Some(15)
        );
    }

    #[test]
    fn recording_formats_and_audio_are_validated() {
        for ext in ["mp4", "webm", "gif"] {
            assert!(
                parse(&[
                    "--record".into(),
                    "1".into(),
                    "--output".into(),
                    format!("out.{ext}")
                ])
                .is_ok()
            );
        }
        assert!(parse(&["--audio-source".into(), "monitor".into()]).is_err());
        assert!(
            parse(&[
                "--record".into(),
                "1".into(),
                "--audio-source".into(),
                "monitor".into(),
                "--output".into(),
                "out.gif".into()
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--record".into(),
                "1".into(),
                "--output".into(),
                "out.avi".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn name_template_expands_capture_metadata() {
        let options = parse(&[
            "--window".into(),
            "--name-template".into(),
            "Capture-{mode}-{width}x{height}-{timestamp}-{nanos}".into(),
        ])
        .unwrap();
        let rect = Rect {
            x: 10,
            y: 20,
            w: 2560,
            h: 1440,
        };
        assert_eq!(
            render_name(
                &options.name_template,
                options.mode,
                rect,
                "png",
                1_700_000_000,
                42,
            )
            .unwrap(),
            "Capture-window-2560x1440-1700000000-000000042.png"
        );
    }

    #[test]
    fn name_template_rejects_paths_and_unknown_placeholders() {
        let rect = Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        for template in ["../escape", "nested/name", "{unknown}", ".."] {
            assert!(render_name(template, Mode::Full, rect, "png", 1, 0).is_err());
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-shot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_file_creates_missing_directories_and_never_overwrites() {
        let dir = scratch("create");
        let path = dir.join("missing/nested/shot.png");
        new_file(&path, b"one").expect("creates parents");
        assert_eq!(fs::read(&path).unwrap(), b"one");
        let again = new_file(&path, b"two").unwrap_err();
        assert!(again.contains("shot.png"), "{again}");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"one",
            "existing capture must survive"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_file_reports_a_parent_that_is_not_a_directory() {
        let dir = scratch("blocked");
        let blocker = dir.join("not-a-dir");
        fs::write(&blocker, b"file").unwrap();
        let error = new_file(&blocker.join("shot.png"), b"x").unwrap_err();
        assert!(error.contains("not-a-dir"), "{error}");
        assert_eq!(fs::read(&blocker).unwrap(), b"file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn region_outside_capture_is_an_error_not_a_blank_image() {
        let image = Image {
            width: 4,
            height: 4,
            rgba: vec![0; 4 * 4 * 4],
        };
        let outside = Rect {
            x: 10,
            y: 10,
            w: 3,
            h: 3,
        };
        assert!(image.clone().crop(outside).is_err());
    }
}
