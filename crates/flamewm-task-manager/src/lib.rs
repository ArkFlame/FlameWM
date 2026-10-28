//! Native `FlameWM` process monitor: one htop-like surface, no resident daemon.
//! /proc sampling happens on one on-demand worker; the X11 reactor sleeps between wakeups.
mod graph;
mod model;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignal, ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::context_menu::menu_surface;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess,
    UiKeyEvent, UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity,
    decode_document, run_with_controller_events_role_and_two_readable_fds,
};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-manager.rwr"));
const ROWS: usize = 40;
const CORES: usize = 32;
const CORES_PER_COLUMN: usize = 8;
const CORE_COLUMNS: usize = CORES / CORES_PER_COLUMN;
const WIDTH: u32 = 1180;
const HEIGHT: u32 = 805;
const ROW_HEIGHT: u32 = 30;
const LIST_TOP: u32 = 50 + 196 + 46 + 1 + 4;
const FIXED_HEIGHT: u32 = LIST_TOP + 30;
const MENU_WIDTH: f32 = 215.0;
const MENU_HEIGHT: f32 = 290.0;
const NICE_LEVELS: [(i32, &str); 5] = [
    (-5, "Above normal priority"),
    (0, "Normal priority"),
    (5, "Below normal priority"),
    (10, "Low priority"),
    (19, "Idle priority"),
];
const CPU_RGB: u32 = 0x003d_aee9;
const MEMORY_RGB: u32 = 0x00b7_7ee0;
const DOWNLOAD_RGB: u32 = 0x004f_d18b;
const UPLOAD_RGB: u32 = 0x00f4_a259;
const HEAT_RGB: u32 = 0x003d_aee9;
const DANGER_RGB: u32 = 0x00ff_858d;
const MIN_NETWORK_SCALE: f32 = 64.0 * 1024.0;

fn visible_rows(height: u32) -> usize {
    (height.saturating_sub(FIXED_HEIGHT) / ROW_HEIGHT).clamp(1, ROWS as u32) as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Cpu,
    Memory,
    Pid,
    Name,
}

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

#[derive(Default)]
struct Histories {
    cpu: graph::History,
    memory: graph::History,
    download: graph::History,
    upload: graph::History,
}

struct Application {
    snapshot: model::Snapshot,
    histories: Histories,
    graphs_dirty: bool,
    theme: SettingsSnapshot,
    theme_dirty: bool,
    sort: Sort,
    descending: bool,
    search_active: bool,
    query: String,
    first: usize,
    selected: Option<(u32, u64)>,
    hovered: Option<usize>,
    rows: Vec<(u32, u64)>,
    context: bool,
    confirm: bool,
    viewport: usize,
    window: (f32, f32),
    message: String,
    actions: SyncSender<ProcessRequest>,
}
impl Application {
    fn new(theme: SettingsSnapshot, actions: SyncSender<ProcessRequest>) -> Self {
        Self {
            snapshot: model::Snapshot::default(),
            histories: Histories::default(),
            graphs_dirty: true,
            theme,
            theme_dirty: true,
            sort: Sort::Cpu,
            descending: true,
            search_active: false,
            query: String::new(),
            first: 0,
            selected: None,
            hovered: None,
            rows: Vec::new(),
            context: false,
            confirm: false,
            viewport: visible_rows(HEIGHT),
            window: (WIDTH as f32, HEIGHT as f32),
            message: "Monitoring active · updates every second".into(),
            actions,
        }
    }
    fn accept(&mut self, snapshot: model::Snapshot) {
        let memory = if snapshot.mem_total == 0 {
            0.0
        } else {
            100.0 * snapshot.mem_used as f32 / snapshot.mem_total as f32
        };
        self.histories.cpu.push(snapshot.cpu_total as f32);
        self.histories.memory.push(memory);
        self.histories.download.push(snapshot.net_rx as f32);
        self.histories.upload.push(snapshot.net_tx as f32);
        self.snapshot = snapshot;
        self.graphs_dirty = true;
    }
    fn sorted(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let mut hits: Vec<usize> = self
            .snapshot
            .processes
            .iter()
            .enumerate()
            .filter(|(_, process)| {
                query.is_empty()
                    || process.name.to_lowercase().contains(&query)
                    || process.command.to_lowercase().contains(&query)
                    || process.pid.to_string().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        hits.sort_unstable_by(|&a, &b| {
            let a = &self.snapshot.processes[a];
            let b = &self.snapshot.processes[b];
            let order = match self.sort {
                Sort::Cpu => a.cpu.total_cmp(&b.cpu),
                Sort::Memory => a.rss_kib.cmp(&b.rss_kib),
                Sort::Pid => a.pid.cmp(&b.pid),
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            let order = if self.descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(|| a.pid.cmp(&b.pid))
        });
        hits
    }
    fn selected_process(&self) -> Option<&model::Process> {
        let (pid, start) = self.selected?;
        self.snapshot
            .processes
            .iter()
            .find(|p| p.pid == pid && p.start == start)
    }
    fn paint(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let accent = accent_color(&self.theme);
        if std::mem::take(&mut self.theme_dirty) {
            self.paint_theme(doc, accent)?;
        }
        self.paint_metrics(doc)?;
        if std::mem::take(&mut self.graphs_dirty) {
            self.paint_graphs(doc)?;
        }
        self.paint_table(doc, accent)?;
        self.paint_menus(doc)
    }
    fn paint_theme(&self, doc: &mut impl UiDocumentAccess, accent: u32) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("task-manager", rgba(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_border(doc, "task-manager", &self.theme)?;
        doc.color_variable("--accent", rgba(accent, 255))?;
        doc.color_variable("--accent-soft", rgba(accent, 42))?;
        doc.color_variable(
            "--border",
            flamewm_ui_x11::surface_style::surface_border(&self.theme),
        )?;
        let menu = menu_surface(&self.theme);
        for id in ["context", "confirm"] {
            doc.background(id, rgba(menu.fill.rgb, menu.fill.alpha.max(242)))?;
            doc.corner_radius(id, menu.radius as f32)?;
        }
        doc.foreground("menu-end-text", rgba(DANGER_RGB, 255))
    }
    fn paint_metrics(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let snapshot = &self.snapshot;
        let count = snapshot.cores.len();
        let columns = count.div_ceil(CORES_PER_COLUMN).clamp(1, CORE_COLUMNS);
        let track = core_track_width(self.window.0, columns);
        for column in 0..CORE_COLUMNS {
            doc.visible(
                &format!("core-col-{column}"),
                column == 0 || column * CORES_PER_COLUMN < count,
            )?;
        }
        for i in 0..CORES {
            let visible = i < count;
            doc.visible(&format!("core-{i}"), visible)?;
            if !visible {
                continue;
            }
            let overflow = i == CORES - 1 && count > CORES;
            let value = if overflow {
                snapshot.cores[i..].iter().sum::<f64>() / (count - i) as f64
            } else {
                snapshot.cores[i]
            }
            .clamp(0.0, 100.0);
            doc.text(
                &format!("core-label-{i}"),
                if overflow {
                    format!("{i}+")
                } else {
                    i.to_string()
                },
            )?;
            doc.text(&format!("core-pct-{i}"), format!("{value:.0}%"))?;
            let fill = format!("cpu-fill-{i}");
            doc.size(&fill, track * value as f32 / 100.0, 7.0)?;
            doc.background(&fill, rgba(load_color(value), 255))?;
        }
        doc.text("cores-count", format!("{count} threads"))?;
        doc.text("cpu-total", format!("{:.0}%", snapshot.cpu_total))?;
        doc.text("cpu-value", format!("{:.0}%", snapshot.cpu_total))?;
        doc.text("cpu-foot", format!("Load {}", snapshot.load))?;
        doc.text(
            "mem-value",
            format!(
                "{:.1} / {:.1} GiB",
                gib(snapshot.mem_used),
                gib(snapshot.mem_total)
            ),
        )?;
        doc.text(
            "swap-text",
            format!(
                "{:.1} / {:.1} GiB",
                gib(snapshot.swap_used),
                gib(snapshot.swap_total)
            ),
        )?;
        doc.size(
            "swap-fill",
            fraction_width(snapshot.swap_used, snapshot.swap_total, 60),
            5.0,
        )?;
        doc.text("net-value", rate(snapshot.net_rx + snapshot.net_tx))?;
        doc.text("net-down", rate(snapshot.net_rx))?;
        doc.text("net-up", rate(snapshot.net_tx))?;
        doc.text(
            "summary",
            format!(
                "{} processes · {} threads · {} running · up {}",
                snapshot.tasks, snapshot.threads, snapshot.running, snapshot.uptime
            ),
        )?;
        doc.text("head-cpu-pct", format!("{:.0}%", snapshot.cpu_total))?;
        doc.text(
            "head-mem-pct",
            format!(
                "{:.0}%",
                fraction_width(snapshot.mem_used, snapshot.mem_total, 100)
            ),
        )
    }
    fn paint_graphs(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let histories = &self.histories;
        let network_scale = histories
            .download
            .peak()
            .max(histories.upload.peak())
            .max(MIN_NETWORK_SCALE);
        let graphs = [
            (
                "cpu-graph",
                graph::render(
                    &[graph::Series {
                        history: &histories.cpu,
                        rgb: CPU_RGB,
                        filled: true,
                    }],
                    100.0,
                ),
            ),
            (
                "mem-graph",
                graph::render(
                    &[graph::Series {
                        history: &histories.memory,
                        rgb: MEMORY_RGB,
                        filled: true,
                    }],
                    100.0,
                ),
            ),
            (
                "net-graph",
                graph::render(
                    &[
                        graph::Series {
                            history: &histories.download,
                            rgb: DOWNLOAD_RGB,
                            filled: true,
                        },
                        graph::Series {
                            history: &histories.upload,
                            rgb: UPLOAD_RGB,
                            filled: false,
                        },
                    ],
                    network_scale,
                ),
            ),
        ];
        for (id, pixels) in graphs {
            doc.image_rgba8(
                id,
                RuntimeImage {
                    source: id.to_owned(),
                    width: graph::WIDTH,
                    height: graph::HEIGHT,
                    pixels,
                },
            )?;
        }
        Ok(())
    }
    fn paint_table(&mut self, doc: &mut impl UiDocumentAccess, accent: u32) -> Result<(), String> {
        let hits = self.sorted();
        let max_first = hits.len().saturating_sub(self.viewport);
        self.first = self.first.min(max_first);
        doc.text(
            "search",
            if self.search_active {
                format!("{}▌", self.query)
            } else if self.query.is_empty() {
                "Search processes".into()
            } else {
                format!("{}  ×", self.query)
            },
        )?;
        doc.text("status", self.message.clone())?;
        for (sort, id, label) in [
            (Sort::Name, "head-name-label", "Name"),
            (Sort::Pid, "head-pid-label", "PID"),
            (Sort::Cpu, "head-cpu-label", "CPU"),
            (Sort::Memory, "head-mem-label", "Memory"),
        ] {
            let arrow = match (self.sort == sort, self.descending) {
                (false, _) => "",
                (true, true) => " ↓",
                (true, false) => " ↑",
            };
            doc.text(id, format!("{label}{arrow}"))?;
        }
        self.rows.clear();
        let total = self.snapshot.mem_total.max(1);
        for slot in 0..ROWS {
            let visible = slot < self.viewport && self.first + slot < hits.len();
            doc.visible(&format!("row-{slot}"), visible)?;
            if !visible {
                continue;
            }
            let p = &self.snapshot.processes[hits[self.first + slot]];
            self.rows.push((p.pid, p.start));
            let id = |name: &str| format!("r-{slot}-{name}");
            doc.text(&id("name"), display_name(p))?;
            doc.background(&id("dot"), rgba(state_color(p.state), 255))?;
            doc.text(&id("pid"), p.pid.to_string())?;
            doc.text(&id("user"), p.user.clone())?;
            doc.text(&id("state"), state_name(p.state).to_owned())?;
            doc.text(&id("threads"), p.threads.to_string())?;
            doc.text(&id("pri"), p.priority.to_string())?;
            doc.text(&id("nice"), p.nice.to_string())?;
            doc.text(&id("virt"), memory_text(p.virt_kib))?;
            doc.text(&id("cpu"), format!("{:.1}%", p.cpu))?;
            doc.text(&id("mem"), memory_text(p.rss_kib))?;
            doc.text(&id("cmd"), p.command.clone())?;
            let memory_share = 100.0 * p.rss_kib as f64 / total as f64;
            doc.background(&id("cpu"), rgba(HEAT_RGB, heat_alpha(p.cpu / 50.0)))?;
            doc.background(&id("mem"), rgba(HEAT_RGB, heat_alpha(memory_share / 10.0)))?;
            let row = format!("row-{slot}");
            if self.selected == Some((p.pid, p.start)) {
                doc.background(&row, rgba(accent, 60))?;
            } else if self.hovered == Some(slot) {
                doc.background(&row, rgba(accent, 32))?;
            } else {
                doc.background_clear(&row)?;
            }
        }
        Ok(())
    }
    fn paint_menus(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        doc.visible("context", self.context)?;
        doc.visible("confirm", self.confirm)?;
        if let Some(process) = self.selected_process() {
            doc.text(
                "context-title",
                format!("{} · PID {}", process.name, process.pid),
            )?;
            for (nice, label) in NICE_LEVELS {
                let mark = if process.nice == nice {
                    "✓  "
                } else {
                    "    "
                };
                doc.text(&format!("menu-nice-{nice}-text"), format!("{mark}{label}"))?;
            }
            doc.text(
                "confirm-text",
                format!("End {} (PID {})?", process.name, process.pid),
            )?;
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
        if let Some(identity) = self.rows.get(row).copied() {
            self.selected = Some(identity);
        }
    }
    fn choose_sort(&mut self, next: Sort) {
        if self.sort == next {
            self.descending = !self.descending;
        } else {
            self.sort = next;
            self.descending = matches!(next, Sort::Cpu | Sort::Memory);
        }
        self.first = 0;
    }
    fn action(
        &mut self,
        event: &UiActionEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.action == "scroll" {
            // Native wheel events carry signed (dx,dy) text; do not scroll for pointer motion.
            let dy = event
                .text
                .as_deref()
                .and_then(|v| v.split_once(','))
                .and_then(|(_, dy)| dy.parse::<f32>().ok())
                .unwrap_or(0.0);
            if dy != 0.0 && event.y >= LIST_TOP as f32 {
                let max = self.sorted().len().saturating_sub(self.viewport);
                if dy > 0.0 {
                    self.first = (self.first + 3).min(max);
                } else {
                    self.first = self.first.saturating_sub(3);
                }
                self.context = false;
                return self.paint(doc);
            }
            return Ok(());
        }
        if matches!(event.phase, UiActionPhase::Hover | UiActionPhase::Motion) {
            let next = event.action.strip_prefix("tm.row.")
                .and_then(|slot| slot.parse::<usize>().ok())
                .filter(|slot| *slot < self.rows.len());
            if next != self.hovered {
                let previous = std::mem::replace(&mut self.hovered, next);
                let accent = accent_color(&self.theme);
                for slot in [previous, next].into_iter().flatten() {
                    let id = format!("row-{slot}");
                    if let Some((pid, start)) = self.rows.get(slot).copied() {
                        if self.selected == Some((pid, start)) {
                            doc.background(&id, rgba(accent, 60))?;
                        } else if next == Some(slot) {
                            doc.background(&id, rgba(accent, 32))?;
                        } else {
                            doc.background_clear(&id)?;
                        }
                    }
                }
            }
            return Ok(());
        }
        if event.phase != UiActionPhase::Release {
            return Ok(());
        }
        let action = event.action.as_str();
        if let Some(slot) = action
            .strip_prefix("tm.row.")
            .and_then(|s| s.parse::<usize>().ok())
        {
            self.choose(slot);
            self.confirm = false;
            self.context = event.button == 3;
            if self.context {
                let (width, height) = self.window;
                doc.position(
                    "context",
                    event.x.min(width - MENU_WIDTH).max(5.0),
                    event.y.min(height - MENU_HEIGHT).max(5.0),
                )?;
            }
        } else {
            match action {
                "tm.sort.cpu" => self.choose_sort(Sort::Cpu),
                "tm.sort.mem" => self.choose_sort(Sort::Memory),
                "tm.sort.pid" => self.choose_sort(Sort::Pid),
                "tm.sort.name" => self.choose_sort(Sort::Name),
                "tm.search" => {
                    self.search_active = true;
                    self.context = false;
                }
                "tm.refresh" => self.message = "Live data refreshes every second".into(),
                "tm.cancel" => {
                    self.context = false;
                    self.confirm = false;
                }
                "tm.terminate" => {
                    self.context = false;
                    self.confirm = self.selected_process().is_some();
                }
                "tm.confirm-terminate" => {
                    self.confirm = false;
                    if let Some((pid, start)) = self.selected {
                        self.enqueue(ProcessRequest::Terminate { pid, start });
                    }
                }
                _ => {
                    if let Some(nice) = action
                        .strip_prefix("tm.nice.")
                        .and_then(|s| s.parse::<i32>().ok())
                    {
                        self.context = false;
                        if let Some((pid, start)) = self.selected {
                            self.enqueue(ProcessRequest::Nice { pid, start, nice });
                        }
                    }
                }
            }
        }
        self.paint(doc)
    }
    fn key(&mut self, key: &UiKeyEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press {
            return Ok(());
        }
        let name = key.key.to_lowercase();
        if self.confirm {
            if name == "escape" {
                self.confirm = false;
            }
            return self.paint(doc);
        }
        if self.search_active {
            match name.as_str() {
                "escape" | "return" | "enter" => self.search_active = false,
                "backspace" => {
                    self.query.pop();
                    self.first = 0;
                }
                _ => {
                    if let Some(text) = &key.text {
                        self.query
                            .extend(text.chars().filter(|c| !c.is_control()).take(8));
                        if self.query.chars().count() > 128 {
                            self.query = self.query.chars().take(128).collect();
                        }
                        self.first = 0;
                    }
                }
            }
        } else {
            match name.as_str() {
                "f3" | "/" => self.search_active = true,
                "escape" => {
                    self.query.clear();
                    self.context = false;
                }
                "f6" => self.choose_sort(match self.sort {
                    Sort::Cpu => Sort::Memory,
                    Sort::Memory => Sort::Pid,
                    Sort::Pid => Sort::Name,
                    Sort::Name => Sort::Cpu,
                }),
                "up" | "down" | "page_up" | "page_down" | "prior" | "next" => {
                    let hits = self.sorted();
                    if hits.is_empty() {
                        return Ok(());
                    }
                    let pos = self
                        .selected
                        .and_then(|identity| {
                            hits.iter().position(|i| {
                                let p = &self.snapshot.processes[*i];
                                (p.pid, p.start) == identity
                            })
                        })
                        .unwrap_or(0);
                    let step = match name.as_str() {
                        "page_up" | "page_down" | "prior" | "next" => self.viewport,
                        _ => 1,
                    };
                    let index = if matches!(name.as_str(), "up" | "page_up" | "prior") {
                        pos.saturating_sub(step)
                    } else {
                        (pos + step).min(hits.len() - 1)
                    };
                    let p = &self.snapshot.processes[hits[index]];
                    self.selected = Some((p.pid, p.start));
                    if index < self.first {
                        self.first = index;
                    } else if index >= self.first + self.viewport {
                        self.first = index + 1 - self.viewport;
                    }
                }
                "f7" | "f8" => {
                    if let Some(p) = self.selected_process() {
                        let (pid, start, target) = (
                            p.pid,
                            p.start,
                            (p.nice + if name == "f7" { -1 } else { 1 }).clamp(-20, 19),
                        );
                        self.enqueue(ProcessRequest::Nice {
                            pid,
                            start,
                            nice: target,
                        });
                    }
                }
                "f9" => self.confirm = self.selected_process().is_some(),
                _ => (),
            }
        }
        self.paint(doc)
    }
}
/// The kernel cuts `comm` to 15 bytes; the executable name from the command line is the full one.
fn display_name(process: &model::Process) -> String {
    const COMM_LIMIT: usize = 15;
    if process.name.len() < COMM_LIMIT {
        return process.name.clone();
    }
    process
        .command
        .split_whitespace()
        .next()
        .and_then(|program| program.rsplit('/').next())
        .filter(|program| program.starts_with(&process.name))
        .map_or_else(|| process.name.clone(), str::to_owned)
}
fn memory_text(kib: u64) -> String {
    if kib >= 1024 * 1024 {
        format!("{:.2} GB", kib as f64 / 1_048_576.0)
    } else {
        format!("{:.1} MB", kib as f64 / 1024.0)
    }
}
fn rate(bytes_per_second: f64) -> String {
    let value = bytes_per_second.max(0.0);
    if value >= 1_048_576.0 {
        format!("{:.1} MiB/s", value / 1_048_576.0)
    } else if value >= 1024.0 {
        format!("{:.1} KiB/s", value / 1024.0)
    } else {
        format!("{value:.0} B/s")
    }
}
fn state_name(state: char) -> &'static str {
    match state {
        'R' => "Running",
        'S' => "Sleeping",
        'D' => "Disk wait",
        'Z' => "Zombie",
        'T' => "Stopped",
        't' => "Traced",
        'I' => "Idle",
        'X' | 'x' => "Dead",
        _ => "Unknown",
    }
}
fn state_color(state: char) -> u32 {
    match state {
        'R' => 0x004f_d18b,
        'D' => 0x00f4_c152,
        'Z' | 'X' | 'x' => 0x00ff_6b6b,
        'T' | 't' => 0x00b7_7ee0,
        _ => 0x007f_8389,
    }
}
/// htop/btop-style meter colour: teal when idle, amber when busy, red when saturated.
fn load_color(percent: f64) -> u32 {
    const STOPS: [(f64, [f64; 3]); 3] = [
        (0.0, [61.0, 219.0, 217.0]),
        (60.0, [244.0, 193.0, 82.0]),
        (100.0, [255.0, 107.0, 107.0]),
    ];
    let value = percent.clamp(0.0, 100.0);
    let upper = STOPS
        .iter()
        .position(|(at, _)| value <= *at)
        .unwrap_or(2)
        .max(1);
    let (low_at, low) = STOPS[upper - 1];
    let (high_at, high) = STOPS[upper];
    let t = (value - low_at) / (high_at - low_at);
    let channel = |index: usize| (low[index] + (high[index] - low[index]) * t).round() as u32;
    channel(0) << 16 | channel(1) << 8 | channel(2)
}
/// Windows 11-style heat cell: a faint tint at rest that deepens with the share in `fraction`.
fn heat_alpha(fraction: f64) -> u8 {
    (14.0 + 116.0 * fraction.clamp(0.0, 1.0)).round() as u8
}
/// Inner width of one core meter track, from the CSS geometry of `.metrics`, `.cores-card`,
/// `.core-col` and `.core`: the flexible tracks share what the fixed cards and labels leave.
fn core_track_width(window_width: f32, columns: usize) -> f32 {
    const GRAPH_CARDS: f32 = 3.0 * 232.0;
    const METRIC_GAPS: f32 = 3.0 * 12.0;
    const METRIC_PADDING: f32 = 2.0 * 16.0;
    const CARD_CHROME: f32 = 2.0 * 12.0 + 2.0 + 2.0;
    const CORE_FIXED: f32 = 16.0 + 30.0 + 2.0 * 6.0;
    const TRACK_CHROME: f32 = 2.0 * 1.0 + 2.0 * 1.0;
    let columns = columns.max(1) as f32;
    let card = window_width - GRAPH_CARDS - METRIC_GAPS - METRIC_PADDING - CARD_CHROME;
    ((card - 14.0 * (columns - 1.0)) / columns - CORE_FIXED - TRACK_CHROME).max(0.0)
}
fn gib(value: u64) -> f64 {
    value as f64 / 1_048_576.0
}
fn fraction_width(used: u64, total: u64, width: u32) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used.min(total) as f64 / total as f64 * f64::from(width)) as f32
    }
}
fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
        a: alpha,
    }
}
fn accent_color(snapshot: &SettingsSnapshot) -> u32 {
    let name = match snapshot.values.get("accent") {
        Some(SettingValue::Text(text)) => text.as_str(),
        _ => "red",
    };
    match name {
        "red" => 0x00ef_4048,
        "blue" => 0x003d_aee9,
        "purple" => 0x009b_59b6,
        "teal" => 0x001a_bc9c,
        "green" => 0x0027_ae60,
        "amber" => 0x00f3_9c12,
        _ => u32::from_str_radix(name.strip_prefix('#').unwrap_or(name), 16)
            .ok()
            .filter(|_| name.trim_start_matches('#').len() == 6)
            .unwrap_or(0x00ef_4048),
    }
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
    let installed = std::env::current_exe().ok().and_then(|exe| {
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
                if !active.load(Ordering::Relaxed) {
                    break;
                }
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
    let outcome = run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 75,
            y: 55,
            title: "FlameWM Task Manager".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-task-manager".into(),
                class: "flamewm-task-manager".into(),
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
                UiControllerEvent::Resized { width, height } => {
                    app.viewport = visible_rows(*height);
                    app.window = (*width as f32, *height as f32);
                    app.paint(doc)
                }
                UiControllerEvent::Drag(_) => Ok(()),
            }
        },
        move |doc| {
            let mut app = wake_window.borrow_mut();
            let mut changed = false;
            if let Some(theme) = incoming.borrow_mut().take() {
                app.theme = theme;
                app.theme_dirty = true;
                changed = true;
            }
            while let Ok(snapshot) = wake_queue.borrow().try_recv() {
                app.accept(snapshot);
                changed = true;
            }
            while let Ok(message) = result_rx.try_recv() {
                app.message = message;
                changed = true;
            }
            if changed {
                app.paint(doc)?;
            }
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
        assert_eq!(accent_color(&snap), 0x00ef_4048);
        assert_eq!(surface_fill(&snap).alpha, 127);
    }
    #[test]
    fn load_colors_run_from_teal_through_amber_to_red() {
        assert_eq!(load_color(0.0), 0x003d_dbd9);
        assert_eq!(load_color(60.0), 0x00f4_c152);
        assert_eq!(load_color(100.0), 0x00ff_6b6b);
        assert_eq!(load_color(250.0), 0x00ff_6b6b);
        assert_eq!(heat_alpha(-1.0), 14);
        assert_eq!(heat_alpha(9.0), 130);
    }
    #[test]
    fn core_tracks_share_the_processor_card() {
        let one = core_track_width(WIDTH as f32, 1);
        let two = core_track_width(WIDTH as f32, 2);
        assert!(one > 2.0 * two);
        assert_eq!(core_track_width(100.0, 4), 0.0);
    }
    #[test]
    fn display_name_restores_names_cut_by_the_kernel() {
        let mut process = model::Process::default();
        process.name = "flamewm-task-ma".into();
        process.command = "/usr/bin/flamewm-task-manager --flag".into();
        assert_eq!(display_name(&process), "flamewm-task-manager");
        process.command = "python3 script.py".into();
        assert_eq!(display_name(&process), "flamewm-task-ma");
        process.name = "bash".into();
        assert_eq!(display_name(&process), "bash");
    }
    #[test]
    fn rates_and_memory_use_readable_units() {
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
        assert_eq!(rate(3.0 * 1_048_576.0), "3.0 MiB/s");
        assert_eq!(memory_text(2048), "2.0 MB");
        assert_eq!(memory_text(3 * 1024 * 1024), "3.00 GB");
    }
    #[test]
    fn default_rows_fit_without_clipping_and_resize_is_bounded() {
        assert_eq!(visible_rows(HEIGHT), 15);
        assert_eq!(visible_rows(0), 1);
        assert_eq!(visible_rows(4096), ROWS);
    }
}
