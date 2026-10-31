//! FlameWM Image Viewer: one glass window showing one image fitted to the window, with zoom, pan and
//! browsing through the images of its folder. Decoding runs on the `decode` worker; this module owns
//! the window state and places the frames the worker returns. Idle means no timers and no polling:
//! the reactor sleeps until a UI event, a worker reply or a settings change arrives.

mod decode;
mod view;

use std::cell::RefCell;
use std::env;
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignal, ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_image_core::ImageSourceFormat;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess,
    UiKeyEvent, UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity,
    decode_document, run_with_controller_events_role_and_two_readable_fds,
};

use decode::{Job, Rect, Reply};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-imageviewer.rwr"));
const WIDTH: u32 = 1000;
const HEIGHT: u32 = 680;
const PHOTO: &str = "photo";
const HELP: &str = "Open an image: flamewm-imageviewer FILE";
const TEXT_RGB: u32 = 0x00f5_f5f7;
const MUTED_RGB: u32 = 0x00a1_a1a6;
const SUBTLE_RGB: u32 = 0x0086_868b;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x0053_9bff;

/// Text, accent and wash colours for the current glass tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    text: u32,
    muted: u32,
    subtle: u32,
    accent: u32,
    /// Base of the translucent key/bar wash; the alpha is applied at paint time.
    wash: u32,
}

impl Palette {
    /// Light text on dark glass, dark text on light glass, and the Settings `accent` colour.
    fn for_theme(theme: &SettingsSnapshot) -> Self {
        let accent = match theme.values.get("accent") {
            Some(SettingValue::Text(hex)) => u32::from_str_radix(hex.trim_start_matches('#'), 16)
                .map_or(ACCENT_RGB, |rgb| rgb & 0x00ff_ffff),
            _ => ACCENT_RGB,
        };
        if luminance(surface_fill(theme).rgb) > 150 {
            Self {
                text: 0x001d_1d1f,
                muted: 0x0051_5154,
                subtle: 0x006e_6e73,
                accent,
                wash: 0,
            }
        } else {
            Self {
                text: TEXT_RGB,
                muted: MUTED_RGB,
                subtle: SUBTLE_RGB,
                accent,
                wash: 0x00ff_ffff,
            }
        }
    }
}

fn luminance(rgb: u32) -> u32 {
    (((rgb >> 16) & 0xff) * 299 + ((rgb >> 8) & 0xff) * 587 + (rgb & 0xff) * 114) / 1000
}

fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: alpha,
    }
}

/// Image files of the folder that holds `path`, sorted by name. Only formats the decoder reads.
fn siblings(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut images: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate.is_file() && ImageSourceFormat::from_extension(candidate).is_some()
        })
        .collect();
    images.sort_by_cached_key(|candidate| {
        candidate
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    images
}

/// The window state. Pixels never live here except inside the document after a frame arrives.
struct App {
    theme: SettingsSnapshot,
    palette: Palette,
    jobs: Sender<Job>,
    /// Images of the folder, in browse order. The opened file is always present.
    images: Vec<PathBuf>,
    index: usize,
    /// Bumped on every image change so late replies for an older image are ignored.
    generation: u64,
    /// Pixel size of the active image once decoded.
    size: Option<(u32, u32)>,
    /// True once a frame of the active image is on screen.
    framed: bool,
    /// Fit mode re-derives the zoom on every resize; any manual zoom leaves it.
    fit: bool,
    zoom: f64,
    /// Top-left of the displayed image relative to the stage, in whole pixels.
    offset: (i64, i64),
    stage: (u32, u32),
    seq: u64,
    /// Sequence and stage geometry of the last viewport request. Only that frame is placed.
    shown: Option<(u64, (u32, u32), (u32, u32))>,
    drag: Option<(f32, f32)>,
    message: String,
}

impl App {
    fn new(theme: SettingsSnapshot, jobs: Sender<Job>, path: Option<&Path>) -> Self {
        let palette = Palette::for_theme(&theme);
        let mut app = Self {
            theme,
            palette,
            jobs,
            images: Vec::new(),
            index: 0,
            generation: 0,
            size: None,
            framed: false,
            fit: true,
            zoom: 1.0,
            offset: (0, 0),
            stage: (WIDTH, HEIGHT),
            seq: 0,
            shown: None,
            drag: None,
            message: HELP.into(),
        };
        if let Some(path) = path {
            app.images = siblings(path);
            if !app.images.iter().any(|image| image == path) {
                app.images.insert(0, path.to_path_buf());
            }
            app.index = app
                .images
                .iter()
                .position(|image| image == path)
                .unwrap_or(0);
            app.load();
        }
        app
    }

    fn send(&self, job: Job) {
        // The worker only stops when this sender drops, so a send error means the window is closing.
        let _ = self.jobs.send(job);
    }

    /// Starts decoding the active image and prefetches its neighbours. Resets the view to fit.
    fn load(&mut self) {
        self.generation += 1;
        self.size = None;
        self.framed = false;
        self.fit = true;
        self.zoom = 1.0;
        self.offset = (0, 0);
        self.shown = None;
        self.drag = None;
        self.message = "Loading…".into();
        let path = self.images[self.index].clone();
        self.send(Job::Load {
            generation: self.generation,
            path,
        });
        let len = self.images.len();
        if len > 1 {
            for forward in [false, true] {
                let neighbour = self.images[view::step_index(len, self.index, forward)].clone();
                self.send(Job::Prefetch { path: neighbour });
            }
        }
    }

    fn navigate(&mut self, forward: bool, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if self.images.len() > 1 {
            self.index = view::step_index(self.images.len(), self.index, forward);
            self.load();
        }
        self.paint(doc)
    }

    fn refit(&mut self) {
        if let (true, Some((w, h))) = (self.fit, self.size) {
            self.zoom = view::fit_zoom(w, h, self.stage);
        }
    }

    /// Recomputes the placement for the current zoom and offset, places the photo node and asks the
    /// worker for the visible part. The old frame is stretched into the new box until the new one arrives.
    fn request_view(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let Some((w, h)) = self.size else {
            return Ok(());
        };
        let display = view::display_size(w, h, self.zoom);
        let placement = view::place(self.stage, display, self.offset);
        self.offset = placement.offset;
        self.seq += 1;
        self.shown = Some((self.seq, placement.at, placement.size));
        doc.position(PHOTO, placement.at.0 as f32, placement.at.1 as f32)?;
        doc.size(PHOTO, placement.size.0 as f32, placement.size.1 as f32)?;
        doc.visible(PHOTO, self.framed)?;
        let path = self.images[self.index].clone();
        let rect = Rect {
            x: placement.src.0,
            y: placement.src.1,
            w: placement.size.0,
            h: placement.size.1,
        };
        self.send(Job::View {
            seq: self.seq,
            path,
            display,
            rect,
        });
        Ok(())
    }

    fn zoom_to(
        &mut self,
        zoom: f64,
        point: (f64, f64),
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let zoom = view::clamp_zoom(zoom);
        if self.size.is_none() || (zoom - self.zoom).abs() < f64::EPSILON {
            return Ok(());
        }
        self.offset = view::zoom_about(self.offset, self.zoom, zoom, point);
        self.zoom = zoom;
        self.fit = false;
        self.request_view(doc)
    }

    fn fit_to_window(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.fit = true;
        self.offset = (0, 0);
        self.refit();
        self.request_view(doc)
    }

    fn resize(
        &mut self,
        width: u32,
        height: u32,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        self.stage = (width.max(1), height.max(1));
        self.refit();
        self.request_view(doc)?;
        self.paint(doc)
    }

    fn reply(&mut self, reply: Reply, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        match reply {
            Reply::Loaded { generation, size } if generation == self.generation => {
                self.size = Some(size);
                self.message.clear();
                self.refit();
                self.request_view(doc)?;
            }
            Reply::Failed {
                generation,
                message,
            } if generation == self.generation => {
                self.size = None;
                self.framed = false;
                self.message = message;
            }
            Reply::ViewFailed { seq, message }
                if self.shown.is_some_and(|(shown, _, _)| shown == seq) =>
            {
                self.message = message;
            }
            Reply::Frame { seq, size, pixels } => {
                let Some((shown, at, _)) = self.shown else {
                    return self.paint(doc);
                };
                if shown != seq {
                    return self.paint(doc);
                }
                doc.image_rgba8(
                    PHOTO,
                    RuntimeImage {
                        source: "imageviewer-frame".into(),
                        width: size.0,
                        height: size.1,
                        pixels,
                    },
                )?;
                doc.position(PHOTO, at.0 as f32, at.1 as f32)?;
                self.framed = true;
                self.message.clear();
            }
            _ => {}
        }
        self.paint(doc)
    }

    fn action(
        &mut self,
        event: &UiActionEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        match (event.action.as_str(), event.phase) {
            ("iv.pan", UiActionPhase::Press) => self.drag = Some((event.x, event.y)),
            ("iv.pan", UiActionPhase::Motion) => {
                if let Some((last_x, last_y)) = self.drag {
                    let dx = (event.x - last_x).round();
                    let dy = (event.y - last_y).round();
                    if dx != 0.0 || dy != 0.0 {
                        self.offset = (self.offset.0 + dx as i64, self.offset.1 + dy as i64);
                        self.drag = Some((last_x + dx, last_y + dy));
                        self.request_view(doc)?;
                    }
                }
            }
            ("iv.pan", UiActionPhase::Release) => self.drag = None,
            // Wheel ticks arrive as "scroll" with text "dx,dy"; a negative dy is wheel up.
            ("scroll", _) => {
                let dy = event
                    .text
                    .as_deref()
                    .and_then(|text| text.split_once(','))
                    .and_then(|(_, dy)| dy.parse::<f32>().ok())
                    .unwrap_or(0.0);
                let point = (f64::from(event.x), f64::from(event.y));
                if dy < 0.0 {
                    self.zoom_to(self.zoom * view::ZOOM_STEP, point, doc)?;
                } else if dy > 0.0 {
                    self.zoom_to(self.zoom / view::ZOOM_STEP, point, doc)?;
                }
            }
            _ => {}
        }
        self.paint(doc)
    }

    fn key(&mut self, key: &UiKeyEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press {
            return Ok(());
        }
        let center = (f64::from(self.stage.0) / 2.0, f64::from(self.stage.1) / 2.0);
        match key.key.as_str() {
            "escape" | "esc" => quit(),
            "left" | "up" | "pageup" => self.navigate(false, doc)?,
            "right" | "down" | "pagedown" | "space" => self.navigate(true, doc)?,
            _ => match key.text.as_deref() {
                Some("+" | "=") => self.zoom_to(self.zoom * view::ZOOM_STEP, center, doc)?,
                Some("-" | "_") => self.zoom_to(self.zoom / view::ZOOM_STEP, center, doc)?,
                Some("1") => self.zoom_to(1.0, center, doc)?,
                Some("0" | "f" | "F") => self.fit_to_window(doc)?,
                _ => {}
            },
        }
        self.paint(doc)
    }

    fn set_theme(
        &mut self,
        theme: SettingsSnapshot,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        self.palette = Palette::for_theme(&theme);
        self.theme = theme;
        self.paint(doc)
    }

    fn meta_text(&self) -> String {
        let position = if self.images.is_empty() {
            String::new()
        } else {
            format!("{}/{}", self.index + 1, self.images.len())
        };
        match self.size {
            Some((w, h)) => {
                let zoom = if self.fit {
                    format!("Fit {:.0}%", self.zoom * 100.0)
                } else {
                    format!("{:.0}%", self.zoom * 100.0)
                };
                format!("{position} · {w}×{h} · {zoom}")
            }
            None => position,
        }
    }

    fn paint(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("imageviewer", rgba(surface.rgb, surface.alpha))?;
        doc.color_variable("--accent", rgba(self.palette.accent, 255))?;
        doc.color_variable("--text", rgba(self.palette.text, 255))?;
        doc.color_variable("--muted", rgba(self.palette.muted, 255))?;
        doc.color_variable("--subtle", rgba(self.palette.subtle, 255))?;
        doc.color_variable("--key", rgba(self.palette.wash, 0x14))?;
        let name = self
            .images
            .get(self.index)
            .and_then(|path| path.file_name())
            .map_or_else(
                || "FlameWM Image Viewer".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        doc.text("name", name)?;
        doc.text("meta", self.meta_text())?;
        doc.text("empty-text", self.message.clone())?;
        doc.visible("empty", !self.framed)?;
        doc.visible(PHOTO, self.framed)
    }
}

fn quit() -> ! {
    std::process::exit(0)
}

fn initial_settings() -> SettingsSnapshot {
    ControlClient::connect(BusKind::Session)
        .ok()
        .and_then(|client| {
            client
                .call_with_timeout(&ControlRequest::GetSettings, Duration::from_millis(250))
                .ok()
        })
        .and_then(|reply| match reply {
            ControlResponse::Settings(snapshot) => Some(snapshot),
            _ => None,
        })
        .unwrap_or_else(|| SettingsSnapshot::new(0))
}

fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let installed = env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr").then(|| usr.join("share/flamewm"))
    });
    installed
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or(source)
}

fn window_backdrop(snapshot: &SettingsSnapshot) -> flamewm_ui_x11::backdrop::WindowBackdrop {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    };
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 80,
    };
    let opacity = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 50,
    };
    let fit = text("wallpaperFit");
    flamewm_ui_x11::backdrop::WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: packaged_root(),
        opacity_percent: opacity,
        blur_percent: blur,
    }
}

pub fn run() -> Result<(), String> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map(|path| fs::canonicalize(&path).unwrap_or(path));
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (reply_tx, reply_rx) = mpsc::channel::<Reply>();
    let (mut reader, mut writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    writer
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    std::thread::Builder::new()
        .name("flamewm-imageviewer-decode".into())
        .spawn(move || decode::run(&job_rx, &reply_tx, &mut writer))
        .map_err(|error| error.to_string())?;

    let app = App::new(theme, job_tx, path.as_deref());
    let initial = app;
    initial.paint(&mut document)?;
    let window = Rc::new(RefCell::new(initial));
    let event_window = Rc::clone(&window);
    let wake_window = Rc::clone(&window);
    let replies = Rc::new(RefCell::new(reply_rx));
    let wake_replies = Rc::clone(&replies);
    let incoming = Rc::new(RefCell::new(None::<SettingsSnapshot>));
    let settings_incoming = Rc::clone(&incoming);
    let settings = ControlSignalClient::connect_session().ok().map(|signals| {
        let fd = signals.watch().fd;
        (
            fd,
            Box::new(move || {
                if signals
                    .on_ready(|signal| {
                        if let ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
                            *settings_incoming.borrow_mut() = Some(snapshot);
                        }
                    })
                    .is_ok()
                {
                    UiWakeDrain::Keep
                } else {
                    UiWakeDrain::Retire
                }
            }) as Box<dyn FnMut() -> UiWakeDrain>,
        )
    });
    let fd = reader.as_raw_fd();
    run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 75,
            y: 55,
            title: "FlameWM Image Viewer".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-imageviewer".into(),
                class: "flamewm-imageviewer".into(),
            }),
        },
        UiWindowRole::Normal,
        fd,
        move || {
            let mut buffer = [0_u8; 128];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return UiWakeDrain::Retire,
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return UiWakeDrain::Keep;
                    }
                    Err(_) => return UiWakeDrain::Retire,
                }
            }
        },
        settings,
        move |event, doc| {
            let mut app = event_window.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action, doc),
                UiControllerEvent::Key(key) => app.key(key, doc),
                UiControllerEvent::Resized { width, height } => app.resize(*width, *height, doc),
                UiControllerEvent::Drag(_) => Ok(()),
                UiControllerEvent::Visibility { .. } => Ok(()),
            }
        },
        move |doc| {
            let mut app = wake_window.borrow_mut();
            if let Some(theme) = incoming.borrow_mut().take() {
                app.set_theme(theme, doc)?;
            }
            loop {
                let next = wake_replies.borrow().try_recv();
                match next {
                    Ok(reply) => app.reply(reply, doc)?,
                    Err(_) => break,
                }
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siblings_are_image_files_sorted_by_name() {
        let dir = env::temp_dir().join(format!("imgv-siblings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["b.PNG", "a.jpg", "notes.txt", "C.gif"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = siblings(&dir.join("a.jpg"))
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.jpg", "b.PNG", "C.gif"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_palette_uses_the_shared_accent() {
        let palette = Palette::for_theme(&SettingsSnapshot::new(0));
        assert_eq!(palette.accent, 0x0053_9bff);
    }
}
