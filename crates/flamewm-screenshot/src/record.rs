//! Screen recording pipeline: one fixed-rate capture worker hands raw RGBA frames to the
//! writer (ffmpeg stdin). The hand-off slot holds one frame. When the writer is busy the worker
//! drops the new frame and skips missed ticks, so memory stays bounded and frames are never queued.

use crate::capture::{self, Image, Rect};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

/// Produces one frame per call. Implemented by X11 capture and by synthetic sources in tests.
pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Image, String>;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames the worker actually grabbed.
    pub captured: u64,
    /// Frames the writer accepted.
    pub written: u64,
    /// Frames discarded because the writer was busy, plus ticks skipped because capture was late.
    pub dropped: u64,
}

pub fn frame_period(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1)))
}

/// Records for `duration` (or until `stop` is set) at `fps`. `make_source` runs on the worker
/// thread, so a source may own a non-`Send` resource such as an X connection.
pub fn run<S, F>(
    make_source: F,
    sink: &mut dyn Write,
    fps: u32,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<Stats, String>
where
    S: FrameSource,
    F: FnOnce() -> Result<S, String> + Send,
{
    let period = frame_period(fps);
    let (tx, rx) = mpsc::sync_channel::<Image>(1);
    thread::scope(|scope| {
        let worker = scope.spawn(move || -> Result<(u64, u64), String> {
            let mut source = make_source()?;
            let start = Instant::now();
            let deadline = start + duration;
            let mut next = start;
            let (mut captured, mut dropped) = (0u64, 0u64);
            loop {
                if Instant::now() >= deadline || stop.load(Ordering::Relaxed) {
                    break;
                }
                let frame = source.next_frame()?;
                captured += 1;
                match tx.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => dropped += 1,
                    Err(TrySendError::Disconnected(_)) => break,
                }
                next += period;
                let now = Instant::now();
                if next > now {
                    thread::sleep(next - now);
                } else {
                    // Capture ran late: skip the ticks already missed instead of bursting to catch up.
                    while next <= now {
                        next += period;
                        dropped += 1;
                    }
                }
            }
            Ok((captured, dropped))
        });

        let mut written = 0u64;
        let mut write_error = None;
        for frame in rx {
            if let Err(e) = sink.write_all(&frame.rgba) {
                write_error = Some(format!("encoder write failed: {e}"));
                break;
            }
            written += 1;
        }
        let worker_result = worker
            .join()
            .map_err(|_| "capture worker panicked".to_owned())?;
        if let Some(error) = write_error {
            return Err(error);
        }
        let (captured, dropped) = worker_result?;
        Ok(Stats {
            captured,
            written,
            dropped,
        })
    })
}

/// Grabs `rect` from the X11 root window on the worker's own connection.
pub struct X11Frames {
    grabber: capture::Grabber,
    rect: Rect,
}

impl FrameSource for X11Frames {
    fn next_frame(&mut self) -> Result<Image, String> {
        self.grabber.grab(self.rect)
    }
}

/// Opens the worker's own X connection when the worker starts.
pub fn x11_frames(rect: Rect) -> impl FnOnce() -> Result<X11Frames, String> + Send {
    move || {
        Ok(X11Frames { grabber: capture::connect_grabber()?, rect })
    }
}

/// Optional encoder check. Recording degrades with a message; screenshots never need it.
pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// ffmpeg arguments that read raw RGBA from stdin and encode to `ext`. The caller appends the output path.
pub fn encoder_args(
    ext: &str,
    fps: u32,
    size: (u32, u32),
    audio: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = [
        "-nostdin", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend([
        "-video_size".into(),
        format!("{}x{}", size.0, size.1),
        "-framerate".into(),
        fps.to_string(),
        "-i".into(),
        "-".into(),
    ]);
    match audio {
        Some(source) => args.extend(["-f", "pulse", "-i", source, "-shortest"].map(str::to_owned)),
        None => args.push("-an".into()),
    }
    match ext {
        "mp4" => args.extend(
            [
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "20", "-pix_fmt", "yuv420p",
            ]
            .map(str::to_owned),
        ),
        "webm" => args.extend(
            ["-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8"].map(str::to_owned),
        ),
        "gif" => args.extend(
            [
                "-filter_complex",
                "[0:v]split[a][b];[a]palettegen[p];[b][p]paletteuse",
            ]
            .map(str::to_owned),
        ),
        other => return Err(format!("unsupported recording format: {other}")),
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frame `n` has its counter in the first byte, so writer order is observable.
    struct Counter {
        next: u8,
        pause: Duration,
    }

    impl FrameSource for Counter {
        fn next_frame(&mut self) -> Result<Image, String> {
            thread::sleep(self.pause);
            let value = self.next;
            self.next = self.next.wrapping_add(1);
            Ok(Image {
                width: 1,
                height: 1,
                rgba: vec![value, 0, 0, 255],
            })
        }
    }

    fn counter(pause: Duration) -> impl FnOnce() -> Result<Counter, String> {
        move || Ok(Counter { next: 0, pause })
    }

    /// Writer that records frame counters and can be made slow.
    struct Recorder {
        seen: Vec<u8>,
        pause: Duration,
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            thread::sleep(self.pause);
            self.seen.push(buf[0]);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn frame_period_is_fixed_and_never_zero() {
        assert_eq!(frame_period(30), Duration::from_nanos(33_333_333));
        assert_eq!(frame_period(0), Duration::from_secs(1));
    }

    #[test]
    fn fast_writer_receives_every_frame_in_capture_order() {
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::ZERO };
        let stop = AtomicBool::new(false);
        let stats = run(counter(Duration::from_millis(5)), &mut sink, 100, Duration::from_millis(150), &stop)
            .expect("recording");
        assert!(stats.written >= 5, "{stats:?}");
        assert_eq!(stats.written as usize, sink.seen.len());
        assert!(sink.seen.windows(2).all(|w| w[1] == w[0].wrapping_add(1)), "out of order: {:?}", sink.seen);
    }

    #[test]
    fn slow_writer_drops_frames_instead_of_queueing() {
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::from_millis(40) };
        let stop = AtomicBool::new(false);
        let stats = run(counter(Duration::ZERO), &mut sink, 200, Duration::from_millis(400), &stop)
            .expect("recording");
        assert!(stats.dropped > 0, "{stats:?}");
        assert!(stats.written < stats.captured, "{stats:?}");
        // One slot plus one in the writer: the writer can never be more than two frames behind.
        let gaps = sink.seen.windows(2).map(|w| w[1].wrapping_sub(w[0])).max().unwrap_or(1);
        assert!(gaps >= 2, "frames must be skipped, not delivered back to back: {:?}", sink.seen);
    }

    #[test]
    fn stop_flag_ends_recording_before_the_first_frame() {
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::ZERO };
        let stop = AtomicBool::new(true);
        let stats = run(counter(Duration::ZERO), &mut sink, 30, Duration::from_secs(10), &stop).expect("stop");
        assert_eq!(stats, Stats::default());
        assert!(sink.seen.is_empty());
    }

    #[test]
    fn duration_bounds_the_recording() {
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::ZERO };
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        run(counter(Duration::ZERO), &mut sink, 60, Duration::from_millis(100), &stop).expect("recording");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn source_failure_is_reported_not_swallowed() {
        struct Broken;
        impl FrameSource for Broken {
            fn next_frame(&mut self) -> Result<Image, String> {
                Err("capture rectangle offscreen".into())
            }
        }
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::ZERO };
        let stop = AtomicBool::new(false);
        let error = run(|| Ok(Broken), &mut sink, 30, Duration::from_secs(5), &stop).unwrap_err();
        assert_eq!(error, "capture rectangle offscreen");
    }

    #[test]
    fn factory_failure_is_reported() {
        let mut sink = Recorder { seen: Vec::new(), pause: Duration::ZERO };
        let stop = AtomicBool::new(false);
        let error = run(
            || Err::<Counter, _>("no X connection".to_owned()),
            &mut sink,
            30,
            Duration::from_secs(5),
            &stop,
        )
        .unwrap_err();
        assert_eq!(error, "no X connection");
    }

    #[test]
    fn writer_failure_stops_capture_promptly() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("pipe closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let error = run(counter(Duration::from_millis(2)), &mut Full, 100, Duration::from_secs(30), &stop)
            .unwrap_err();
        assert!(error.contains("pipe closed"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn mp4_webm_and_gif_use_their_codecs() {
        let mp4 = encoder_args("mp4", 30, (1920, 1080), None).unwrap().join(" ");
        assert!(mp4.contains("-video_size 1920x1080") && mp4.contains("libx264") && mp4.contains("yuv420p"));
        assert!(mp4.ends_with("-an -c:v libx264 -preset ultrafast -crf 20 -pix_fmt yuv420p"), "{mp4}");
        let webm = encoder_args("webm", 30, (640, 480), None).unwrap().join(" ");
        assert!(webm.contains("libvpx-vp9") && webm.contains("realtime"));
        let gif = encoder_args("gif", 15, (640, 480), None).unwrap().join(" ");
        assert!(gif.contains("palettegen") && gif.contains("-framerate 15"));
        assert!(!gif.contains("libx264"));
    }

    #[test]
    fn audio_source_is_opt_in_and_stops_with_video() {
        let off = encoder_args("mp4", 30, (64, 64), None).unwrap();
        assert!(off.contains(&"-an".to_owned()));
        let on = encoder_args("mp4", 30, (64, 64), Some("alsa_output.monitor")).unwrap();
        assert!(!on.contains(&"-an".to_owned()));
        assert!(on.windows(2).any(|w| w == ["-i", "alsa_output.monitor"]));
        assert!(on.contains(&"-shortest".to_owned()));
    }

    #[test]
    fn unknown_format_is_rejected_before_encoding() {
        assert!(encoder_args("avi", 30, (64, 64), None).is_err());
    }
}
