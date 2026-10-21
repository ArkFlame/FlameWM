//! FlameWM Screenshot: an on-demand, native Rust/X11 capture, selection,
//! annotation, export and clipboard utility. No Qt/GTK/WebView process.

mod capture;
mod clipboard;
mod ui;

use capture::{Image, Rect};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use x11rb::connection::Connection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Full,
    Screen,
    Region,
    Active,
    Window,
}
#[derive(Debug)]
struct Options {
    mode: Mode,
    save: bool,
    copy: bool,
    annotate: bool,
    cursor: bool,
    output: Option<PathBuf>,
    delay: Duration,
    record: Option<u64>,
    fps: u32,
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
            delay: Duration::ZERO,
            record: None,
            fps: 30,
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
  --delay SEC      Capture delay, 0..30 seconds\n\
  --cursor         Include mouse pointer in saved image (if XFixes available)\n\
  --record SEC     Record X11 screen/region/window to MP4, 1..3600 seconds;\n\
                   requires optional ffmpeg with x11grab + libx264 support\n\
  --fps N          Recording frame rate, 1..60 (default 30)\n\
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
            "--output" | "--delay" | "--record" | "--fps" => {
                let kind = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {kind}"))?;
                match kind {
                    "--output" => {
                        o.output = Some(PathBuf::from(value));
                        o.save = true
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
        if o.output
            .as_ref()
            .is_some_and(|p| p.extension().and_then(|s| s.to_str()) != Some("mp4"))
        {
            return Err("video output path must use .mp4".into());
        }
    }
    Ok(o)
}

fn pictures_dir() -> PathBuf {
    // XDG user-dirs is intentionally not parsed as shell: unsafe interpolation must never execute.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
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

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}
fn default_output(extension: &str) -> Result<PathBuf, String> {
    let dir = pictures_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!(
        "FlameWM-{}-{n}.{extension}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        n = now_nanos() % 1_000_000_000
    )))
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
            let full_image = capture::grab(conn, screen, full)?;
            let Some(rect) = ui::select_region(conn, screen, &full_image)? else {
                return Err("cancelled".into());
            };
            Ok((rect, Some(full_image)))
        }
    }
}

fn record_video(
    rect: Rect,
    display: &str,
    duration: u64,
    fps: u32,
    path: &Path,
) -> Result<(), String> {
    if path.exists() {
        return Err(format!("output already exists: {}", path.display()));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let input = format!("{display}+{},{}", rect.x, rect.y);
    // Optional on-demand encoder; no recorder daemon, Qt, or GStreamer dependency.
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "x11grab",
            "-framerate",
        ])
        .arg(fps.to_string())
        .arg("-video_size")
        .arg(format!("{}x{}", rect.w, rect.h))
        .arg("-i")
        .arg(input)
        .arg("-t")
        .arg(duration.to_string())
        .args([
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-n",
        ])
        .arg(path)
        .status()
        .map_err(|e| format!("ffmpeg unavailable: {e}"))?;
    if !status.success() {
        return Err(format!("recording failed: ffmpeg exited {status}"));
    }
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
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connection failed: {e}"))?;
    if o.delay > Duration::ZERO {
        std::thread::sleep(o.delay);
    }
    let (rect, full_image) = match selection(&conn, screen, o.mode) {
        Ok(found) => found,
        Err(e) if e == "cancelled" => return Ok(()),
        Err(e) => return Err(e),
    };
    if let Some(duration) = o.record {
        let path = o.output.map_or_else(|| default_output("mp4"), Ok)?;
        record_video(
            rect,
            &std::env::var("DISPLAY").map_err(|_| "DISPLAY must be set")?,
            duration,
            o.fps,
            &path,
        )?;
        println!("{}", path.display());
        return Ok(());
    }
    let mut captured = if let Some(full) = full_image {
        full.crop(rect)?
    } else {
        capture::grab(&conn, screen, rect)?
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
    let bytes = captured.png()?;
    let mut saved = None;
    if o.save {
        let path = o.output.map_or_else(|| default_output("png"), Ok)?;
        new_file(&path, &bytes)?;
        println!("{}", path.display());
        saved = Some(path);
    }
    if o.copy {
        let temporary = saved.is_none();
        let path = if let Some(p) = saved {
            p
        } else {
            let p = std::env::temp_dir().join(format!(
                "flamewm-clipboard-{}-{}.png",
                std::process::id(),
                now_nanos()
            ));
            new_file(&p, &bytes)?;
            p
        };
        clipboard::spawn_owner(&path, temporary)?;
        eprintln!("FlameWM: image copied to clipboard");
    }
    Ok(())
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
}
