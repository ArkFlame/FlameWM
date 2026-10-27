//! Native FlameWM process monitor: one htop-like surface, no resident daemon.
//! /proc sampling happens on one on-demand worker; the X11 reactor sleeps between wakeups.
mod model;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignal, ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess,
    UiKeyEvent, UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole,
    X11WindowIdentity, decode_document, run_with_controller_events_role_and_two_readable_fds,
};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-manager.rwr"));
const ROWS: usize = 22;
const CORES: usize = 16;
const WIDTH: u32 = 1130;
const HEIGHT: u32 = 805;
const ROW_HEIGHT: u32 = 25;
const FIXED_HEIGHT: u32 = 49 + 169 + 42 + 30 + 34;

fn visible_rows(height: u32) -> usize {
    (height.saturating_sub(FIXED_HEIGHT) / ROW_HEIGHT).clamp(1, ROWS as u32) as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort { Cpu, Memory, Pid }

// Only the existing on-demand sampler may execute process-control commands.
// UI callbacks enqueue requests without blocking X11 input or document rendering.
enum ProcessRequest {
    Nice { pid: u32, start: u64, nice: i32 },
    Terminate { pid: u32, start: u64 },
}
impl ProcessRequest {
    fn execute(self) -> String {
        match self {
            Self::Nice { pid, start, nice } => match model::change_nice(pid, start, nice) {
                Ok(()) => format!("PID {pid} priority changed to nice {nice}"),
                Err(error) => format!("Cannot change PID {pid}: {error}"),
            },
            Self::Terminate { pid, start } => match model::terminate(pid, start) {
                Ok(()) => format!("SIGTERM sent to {pid}"),
                Err(error) => format!("Cannot terminate {pid}: {error}"),
            },
        }
    }
}

struct Application {
    snapshot: model::Snapshot,
    theme: SettingsSnapshot,
    sort: Sort,
    descending: bool,
    search_active: bool,
    query: String,
    first: usize,
    selected: Option<(u32, u64)>,
    rows: Vec<(u32, u64)>,
    context: bool,
    confirm: bool,
    viewport: usize,
    message: String,
    actions: SyncSender<ProcessRequest>,
}
impl Application {
    fn new(theme: SettingsSnapshot, actions: SyncSender<ProcessRequest>) -> Self {
        Self {
            snapshot: model::Snapshot::default(), theme, sort: Sort::Cpu,
            descending: true, search_active: false, query: String::new(), first: 0,
            selected: None, rows: Vec::new(), context: false, confirm: false,
            viewport: visible_rows(HEIGHT), message: "Monitoring active · updates every second".into(),
            actions,
        }
    }
    fn sorted(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let mut hits: Vec<usize> = self.snapshot.processes.iter().enumerate()
            .filter(|(_, process)| query.is_empty()
                || process.name.to_lowercase().contains(&query)
                || process.command.to_lowercase().contains(&query)
                || process.pid.to_string().contains(&query))
            .map(|(index, _)| index).collect();
        hits.sort_unstable_by(|&a, &b| {
            let a = &self.snapshot.processes[a];
            let b = &self.snapshot.processes[b];
            let order = match self.sort {
                Sort::Cpu => a.cpu.total_cmp(&b.cpu),
                Sort::Memory => a.rss_kib.cmp(&b.rss_kib),
                Sort::Pid => a.pid.cmp(&b.pid),
            };
            let order = if self.descending { order.reverse() } else { order };
            order.then_with(|| a.pid.cmp(&b.pid))
        });
        hits
    }
    fn selected_process(&self) -> Option<&model::Process> {
        let (pid, start) = self.selected?;
        self.snapshot.processes.iter().find(|p| p.pid == pid && p.start == start)
    }
    fn paint(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("task-manager", rgba(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_border(doc, "task-manager", &self.theme)?;
        let accent = accent_color(&self.theme);
        doc.color_variable("--accent", rgba(accent, 255))?;
        doc.color_variable("--accent-soft", rgba(accent, 42))?;
        doc.color_variable("--card", rgba(0xffffff, 21))?;
        doc.color_variable("--border", flamewm_ui_x11::surface_style::surface_border(&self.theme))?;
        doc.color_variable("--muted", rgba(0xb1b3bb, 255))?;
        for i in 0..CORES {
            let visible = i < self.snapshot.cores.len();
            doc.visible(&format!("core-{i}"), visible)?;
            if visible {
                let overflow = i == CORES - 1 && self.snapshot.cores.len() > CORES;
                let value = if overflow {
                    self.snapshot.cores[i..].iter().sum::<f64>()
                        / (self.snapshot.cores.len() - i) as f64
                } else { self.snapshot.cores[i] }.clamp(0.0, 100.0);
                doc.text(&format!("core-label-{i}"), if overflow {
                    format!("CPU {i}+")
                } else { format!("CPU {i}") })?;
                doc.text(&format!("core-pct-{i}"), format!("{value:.0}%"))?;
                doc.size(&format!("cpu-fill-{i}"), (125.0 * value / 100.0) as f32, 6.0)?;
            }
        }
        let (mt, mu, st, su) = (self.snapshot.mem_total, self.snapshot.mem_used,
            self.snapshot.swap_total, self.snapshot.swap_used);
        doc.text("mem-text", format!("{:.1} / {:.1} GiB", gib(mu), gib(mt)))?;
        doc.text("swap-text", format!("{:.1} / {:.1} GiB", gib(su), gib(st)))?;
        doc.size("mem-fill", fraction_width(mu, mt, 240), 7.0)?;
        doc.size("swap-fill", fraction_width(su, st, 240), 7.0)?;
        doc.text("live-label", format!("● CPU {:.0}% · LIVE 1s", self.snapshot.cpu_total))?;
        doc.text("task-summary", format!("{} tasks · {} threads · {} running",
            self.snapshot.tasks, self.snapshot.threads, self.snapshot.running))?;
        doc.text("load-summary", format!("Load  {}", self.snapshot.load))?;
        doc.text("uptime-summary", format!("Up  {}", self.snapshot.uptime))?;
        let hits = self.sorted();
        let max_first = hits.len().saturating_sub(self.viewport);
        self.first = self.first.min(max_first);
        doc.text("count", format!("{} shown / {} total", hits.len(), self.snapshot.tasks))?;
        doc.text("search", if self.search_active {
            format!("/ {}▌", self.query)
        } else if self.query.is_empty() { "/ Search".into() }
        else { format!("/ {}  ×", self.query) })?;
        doc.text("status", self.message.clone())?;
        doc.text("sort-cpu", format!("CPU {}", if self.sort == Sort::Cpu { if self.descending { "↓" } else { "↑" } } else { "" }))?;
        doc.text("sort-mem", format!("MEM {}", if self.sort == Sort::Memory { if self.descending { "↓" } else { "↑" } } else { "" }))?;
        doc.text("sort-pid", format!("PID {}", if self.sort == Sort::Pid { if self.descending { "↓" } else { "↑" } } else { "" }))?;
        self.rows.clear();
        let total = self.snapshot.mem_total.max(1);
        for slot in 0..ROWS {
            let visible = slot < self.viewport && self.first + slot < hits.len();
            doc.visible(&format!("row-{slot}"), visible)?;
            if !visible { continue; }
            let p = &self.snapshot.processes[hits[self.first + slot]];
            self.rows.push((p.pid, p.start));
            let id = |name: &str| format!("r-{slot}-{name}");
            doc.text(&id("pid"), p.pid.to_string())?;
            doc.text(&id("user"), p.user.clone())?;
            doc.text(&id("pri"), p.priority.to_string())?;
            doc.text(&id("nice"), p.nice.to_string())?;
            doc.text(&id("virt"), kib(p.virt_kib))?;
            doc.text(&id("rss"), kib(p.rss_kib))?;
            doc.text(&id("cpu"), format!("{:.1}", p.cpu))?;
            doc.text(&id("mem"), format!("{:.1}", 100.0 * p.rss_kib as f64 / total as f64))?;
            doc.text(&id("state"), p.state.to_string())?;
            doc.text(&id("name"), p.command.clone())?;
            doc.background(&format!("row-{slot}"), if self.selected == Some((p.pid, p.start)) {
                rgba(accent, 54)
            } else if slot % 2 == 0 { rgba(0xffffff, 8) } else { rgba(0, 0) })?;
        }
        doc.visible("context", self.context)?;
        doc.visible("confirm", self.confirm)?;
        if let Some(process) = self.selected_process() {
            doc.text("context-title", format!("{} · PID {} · nice {}", process.name, process.pid, process.nice))?;
            doc.text("confirm-text", format!("Terminate {} (PID {})?", process.name, process.pid))?;
        }
        Ok(())
    }
    fn enqueue(&mut self, request: ProcessRequest) {
        self.message = if self.actions.try_send(request).is_ok() {
            "Applying process action…".into()
        } else {
            "Process action queue is full or unavailable".into()
        };
    }
    fn choose(&mut self, row: usize) {
        if let Some(identity) = self.rows.get(row).copied() { self.selected = Some(identity); }
    }
    fn choose_sort(&mut self, next: Sort) {
        if self.sort == next { self.descending = !self.descending; }
        else { self.sort = next; self.descending = next != Sort::Pid; }
        self.first = 0;
    }
    fn action(&mut self, event: &UiActionEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if event.action == "scroll" {
            // Native wheel events carry signed (dx,dy) text; do not scroll for pointer motion.
            let dy = event.text.as_deref().and_then(|v| v.split_once(','))
                .and_then(|(_, dy)| dy.parse::<f32>().ok()).unwrap_or(0.0);
            if dy != 0.0 && event.y >= (FIXED_HEIGHT - 34) as f32 {
                let max = self.sorted().len().saturating_sub(self.viewport);
                if dy > 0.0 { self.first = (self.first + 3).min(max); }
                else { self.first = self.first.saturating_sub(3); }
                self.context = false;
                return self.paint(doc);
            }
            return Ok(());
        }
        if event.phase != UiActionPhase::Release { return Ok(()); }
        let action = event.action.as_str();
        if let Some(slot) = action.strip_prefix("tm.row.").and_then(|s| s.parse::<usize>().ok()) {
            self.choose(slot);
            self.confirm = false;
            self.context = event.button == 3;
            if self.context {
                doc.position("context", event.x.min(880.0).max(5.0), event.y.min(550.0).max(5.0))?;
            }
        } else {
            match action {
                "tm.sort.cpu" => self.choose_sort(Sort::Cpu),
                "tm.sort.mem" => self.choose_sort(Sort::Memory),
                "tm.sort.pid" => self.choose_sort(Sort::Pid),
                "tm.search" => { self.search_active = true; self.context = false; },
                "tm.refresh" => self.message = "Live data refreshes every second".into(),
                "tm.cancel" => { self.context = false; self.confirm = false; },
                "tm.terminate" => { self.context = false; self.confirm = self.selected_process().is_some(); },
                "tm.confirm-terminate" => {
                    self.confirm = false;
                    if let Some((pid, start)) = self.selected {
                        self.enqueue(ProcessRequest::Terminate { pid, start });
                    }
                },
                _ => {
                    if let Some(nice) = action.strip_prefix("tm.nice.").and_then(|s| s.parse::<i32>().ok()) {
                        self.context = false;
                        if let Some((pid, start)) = self.selected {
                            self.enqueue(ProcessRequest::Nice { pid, start, nice });
                        }
                    }
                },
            }
        }
        self.paint(doc)
    }
    fn key(&mut self, key: &UiKeyEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press { return Ok(()); }
        let name = key.key.to_lowercase();
        if self.confirm {
            if name == "escape" { self.confirm = false; }
            return self.paint(doc);
        }
        if self.search_active {
            match name.as_str() {
                "escape" | "return" | "enter" => self.search_active = false,
                "backspace" => { self.query.pop(); self.first = 0; },
                _ => if let Some(text) = &key.text {
                    self.query.extend(text.chars().filter(|c| !c.is_control()).take(8));
                    if self.query.chars().count() > 128 {
                        self.query = self.query.chars().take(128).collect();
                    }
                    self.first = 0;
                },
            }
        } else {
            match name.as_str() {
                "f3" | "/" => self.search_active = true,
                "escape" => { self.query.clear(); self.context = false; },
                "f6" => self.choose_sort(match self.sort {
                    Sort::Cpu => Sort::Memory, Sort::Memory => Sort::Pid, Sort::Pid => Sort::Cpu,
                }),
                "up" | "down" | "page_up" | "page_down" | "prior" | "next" => {
                    let hits = self.sorted();
                    if hits.is_empty() { return Ok(()); }
                    let pos = self.selected.and_then(|identity| hits.iter().position(|i| {
                        let p = &self.snapshot.processes[*i]; (p.pid, p.start) == identity
                    })).unwrap_or(0);
                    let step = match name.as_str() {
                        "page_up" | "page_down" | "prior" | "next" => self.viewport,
                        _ => 1,
                    };
                    let index = if matches!(name.as_str(), "up" | "page_up" | "prior") {
                        pos.saturating_sub(step)
                    } else { (pos + step).min(hits.len() - 1) };
                    let p = &self.snapshot.processes[hits[index]];
                    self.selected = Some((p.pid, p.start));
                    if index < self.first { self.first = index; }
                    else if index >= self.first + self.viewport {
                        self.first = index + 1 - self.viewport;
                    }
                },
                "f7" | "f8" => {
                    if let Some(p) = self.selected_process() {
                        let (pid, start, target) = (p.pid, p.start,
                            (p.nice + if name == "f7" { -1 } else { 1 }).clamp(-20, 19));
                        self.enqueue(ProcessRequest::Nice { pid, start, nice: target });
                    }
                },
                "f9" => self.confirm = self.selected_process().is_some(),
                _ => (),
            }
        }
        self.paint(doc)
    }
}
fn kib(value: u64) -> String {
    if value >= 1024 * 1024 { format!("{:.1}G", value as f64 / 1048576.0) }
    else if value >= 1024 { format!("{:.1}M", value as f64 / 1024.0) }
    else { format!("{value}K") }
}
fn gib(value: u64) -> f64 { value as f64 / 1048576.0 }
fn fraction_width(used: u64, total: u64, width: u32) -> f32 {
    if total == 0 { 0.0 } else { (used.min(total) as f64 / total as f64 * width as f64) as f32 }
}
fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: alpha }
}
fn accent_color(snapshot: &SettingsSnapshot) -> u32 {
    let name = match snapshot.values.get("accent") {
        Some(SettingValue::Text(text)) => text.as_str(), _ => "red",
    };
    match name {
        "red" => 0x00ef4048, "blue" => 0x003daee9,
        "purple" => 0x009b59b6, "teal" => 0x001abc9c,
        "green" => 0x0027ae60, "amber" => 0x00f39c12,
        _ => u32::from_str_radix(name.strip_prefix('#').unwrap_or(name), 16)
            .ok().filter(|_| name.trim_start_matches('#').len() == 6).unwrap_or(0x00ef4048),
    }
}

fn initial_settings() -> SettingsSnapshot {
    ControlClient::connect(BusKind::Session).ok()
        .and_then(|client| client.call_with_timeout(&ControlRequest::GetSettings, Duration::from_millis(250)).ok())
        .and_then(|reply| match reply { ControlResponse::Settings(snapshot) => Some(snapshot), _ => None })
        .unwrap_or_else(|| SettingsSnapshot::new(0))
}

fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let installed = std::env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr")
            .then(|| usr.join("share/flamewm"))
    });
    installed.filter(|root| root.join("assets").is_dir()).unwrap_or(source)
}
fn window_backdrop(snapshot: &SettingsSnapshot) -> flamewm_ui_x11::backdrop::WindowBackdrop {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(), _ => String::new(),
    };
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(value)) => (*value).min(100), _ => 80,
    };
    let opacity = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100), _ => 50,
    };
    let fit = text("wallpaperFit");
    flamewm_ui_x11::backdrop::WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: packaged_root(), opacity_percent: opacity, blur_percent: blur,
    }
}

pub fn run() -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let (action_tx, action_rx) = mpsc::sync_channel::<ProcessRequest>(8);
    let (result_tx, result_rx) = mpsc::sync_channel::<String>(8);
    let mut app = Application::new(theme, action_tx);
    app.paint(&mut document)?;
    let (mut reader, mut writer) = UnixStream::pair().map_err(|e| e.to_string())?;
    reader.set_nonblocking(true).map_err(|e| e.to_string())?;
    writer.set_nonblocking(true).map_err(|e| e.to_string())?;
    let (sender, receiver) = mpsc::sync_channel::<model::Snapshot>(1);
    let running = Arc::new(AtomicBool::new(true));
    let active = Arc::clone(&running);
    // One sampler exists only while this window is open; never a permanent desktop daemon.
    let worker = std::thread::spawn(move || {
        let mut sampler = model::Sampler::default();
        while active.load(Ordering::Relaxed) {
            let snapshot = sampler.collect();
            if sender.try_send(snapshot).is_ok() {
                match writer.write(&[1]) {
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(_) => break,
                }
            }
            for _ in 0..10 {
                if !active.load(Ordering::Relaxed) { break; }
                // Process operations may block, but only on this sampler worker.
                // The caller-selected PID/start-time pair is checked by the operation.
                while let Ok(command) = action_rx.try_recv() {
                    let _ = result_tx.try_send(command.execute());
                    let _ = writer.write(&[1]);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    let queue = Rc::new(RefCell::new(receiver));
    let window = Rc::new(RefCell::new(app));
    let event_window = Rc::clone(&window);
    let wake_window = Rc::clone(&window);
    let wake_queue = Rc::clone(&queue);
    let incoming = Rc::new(RefCell::new(None::<SettingsSnapshot>));
    let settings_incoming = Rc::clone(&incoming);
    let settings = ControlSignalClient::connect_session().ok().map(|signals| {
        let fd = signals.watch().fd;
        (fd, Box::new(move || {
            if signals.on_ready(|signal| {
                if let ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
                    *settings_incoming.borrow_mut() = Some(snapshot);
                }
            }).is_ok() { UiWakeDrain::Keep } else { UiWakeDrain::Retire }
        }) as Box<dyn FnMut() -> UiWakeDrain>)
    });
    let fd = reader.as_raw_fd();
    let outcome = run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig { width: WIDTH, height: HEIGHT, x: 75, y: 55,
            title: "FlameWM Task Manager".into(),
            identity: Some(X11WindowIdentity { instance: "flamewm-task-manager".into(),
                class: "flamewm-task-manager".into() }),
        },
        UiWindowRole::Normal,
        fd,
        move || {
            let mut buffer = [0_u8; 128];
            loop { match reader.read(&mut buffer) {
                Ok(0) => return UiWakeDrain::Retire,
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return UiWakeDrain::Keep,
                Err(_) => return UiWakeDrain::Retire,
            }}
        }, settings,
        move |event, doc| {
            let mut app = event_window.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action, doc),
                UiControllerEvent::Key(key) => app.key(key, doc),
                UiControllerEvent::Resized { height, .. } => {
                    app.viewport = visible_rows(*height);
                    app.paint(doc)
                },
                UiControllerEvent::Drag(_) => Ok(()),
            }
        },
        move |doc| {
            let mut app = wake_window.borrow_mut();
            let mut changed = false;
            if let Some(theme) = incoming.borrow_mut().take() { app.theme = theme; changed = true; }
            loop { match wake_queue.borrow().try_recv() {
                Ok(snapshot) => { app.snapshot = snapshot; changed = true; },
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }}
            while let Ok(message) = result_rx.try_recv() {
                app.message = message;
                changed = true;
            }
            if changed { app.paint(doc)?; }
            Ok(())
        },
    );
    running.store(false, Ordering::Relaxed);
    let _ = worker.join();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_progress_is_bounded_in_all_cases() {
        assert_eq!(fraction_width(200, 100, 240), 240.0);
        assert_eq!(fraction_width(0, 0, 240), 0.0);
        assert_eq!(fraction_width(50, 100, 240), 120.0);
    }
    #[test]
    fn default_accent_and_surface_match_flamewm_settings() {
        let snap = SettingsSnapshot::new(0);
        assert_eq!(accent_color(&snap), 0x00ef4048);
        assert_eq!(surface_fill(&snap).alpha, 127);
    }
    #[test]
    fn default_rows_fit_without_clipping_and_resize_is_bounded() {
        assert_eq!(visible_rows(HEIGHT), 19);
        assert_eq!(visible_rows(0), 1);
        assert_eq!(visible_rows(4096), ROWS);
    }
}
