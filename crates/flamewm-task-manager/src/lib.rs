//! Native `FlameWM` process monitor: one htop-like surface, no resident daemon.
//! /proc sampling happens on one on-demand worker; the X11 reactor sleeps between wakeups.
mod graph;
mod model;
mod sample_gate;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_dbus::{ControlSignal, ControlSignalClient, initial_settings};
use flamewm_ui_core::context_menu::menu_surface;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiKeyEvent,
    UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document,
    run_with_controller_events_role_and_two_readable_fds,
};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-manager.rwr"));
const ROWS: usize = 40;
const CORES: usize = 32;
const CORES_PER_COLUMN: usize = 8;
const CORE_COLUMNS: usize = CORES / CORES_PER_COLUMN;
const WIDTH: u32 = 1180;
const HEIGHT: u32 = 805;
const ROW_HEIGHT: u32 = 30;
/// Pre-indented tree labels: index is the clamped depth, so rows never allocate padding.
const INDENTS: [&str; 9] = [
    "",
    "  ",
    "    ",
    "      ",
    "        ",
    "          ",
    "            ",
    "              ",
    "                ",
];
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
    /// Metrics text and core meters change only with a new sample or a resize.
    metrics_dirty: bool,
    sort: Sort,
    descending: bool,
    search_active: bool,
    query: String,
    first: usize,
    selected: Option<(u32, u64)>,
    hovered: Option<usize>,
    rows: Vec<(u32, u64)>,
    /// Matching process indices in display order. Rebuilt only when the snapshot, the sort or the
    /// query changes, so a repaint for hover, a click or a scroll never re-sorts the whole table.
    hits: Vec<usize>,
    hits_dirty: bool,
    /// Two ping-pong raster buffers per graph card: `image_rgba8_shared` keeps the last raster
    /// alive, so the next sample draws into the uniquely-owned alternate instead of a new `Vec`.
    graph_rasters: [[Arc<Vec<u8>>; 2]; 3],
    graph_phase: usize,
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
            metrics_dirty: true,
            sort: Sort::Cpu,
            descending: true,
            search_active: false,
            query: String::new(),
            first: 0,
            selected: None,
            hovered: None,
            rows: Vec::new(),
            hits: Vec::new(),
            hits_dirty: true,
            graph_rasters: [
                [Arc::default(), Arc::default()],
                [Arc::default(), Arc::default()],
                [Arc::default(), Arc::default()],
            ],
            graph_phase: 0,
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
        self.hits_dirty = true;
        self.graphs_dirty = true;
        self.metrics_dirty = true;
    }
    /// Matching processes in display order, rebuilt only after the snapshot, sort or query changed.
    fn ensure_hits(&mut self) {
        if !self.hits_dirty {
            return;
        }
        self.hits_dirty = false;
        let query = self.query.to_lowercase();
        let sort = self.sort;
        let descending = self.descending;
        let processes = &self.snapshot.processes;
        // Name keys are lowercased once per process per rebuild, not twice per comparison.
        let lower_names: Vec<String> = if sort == Sort::Name {
            processes.iter().map(|p| p.name.to_lowercase()).collect()
        } else {
            Vec::new()
        };
        self.hits.clear();
        self.hits.extend(
            processes
                .iter()
                .enumerate()
                .filter(|(_, process)| process_matches(process, &query))
                .map(|(index, _)| index),
        );
        self.hits.sort_unstable_by(|&ai, &bi| {
            let a = &processes[ai];
            let b = &processes[bi];
            let order = match sort {
                Sort::Cpu => a.cpu.total_cmp(&b.cpu),
                Sort::Memory => a.rss_kib.cmp(&b.rss_kib),
                Sort::Pid => a.pid.cmp(&b.pid),
                Sort::Name => lower_names[ai].cmp(&lower_names[bi]),
            };
            let order = if descending { order.reverse() } else { order };
            order.then_with(|| a.pid.cmp(&b.pid))
        });
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
        if std::mem::take(&mut self.metrics_dirty) {
            self.paint_metrics(doc)?;
        }
        if std::mem::take(&mut self.graphs_dirty) {
            self.paint_graphs(doc)?;
        }
        self.paint_table(doc, accent)?;
        self.paint_menus(doc)
    }
    fn paint_theme(&self, doc: &mut impl UiDocumentAccess, accent: u32) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background(
            "task-manager",
            UiColor::from_rgb(surface.rgb, surface.alpha),
        )?;
        flamewm_ui_x11::surface_style::project_surface_outline(doc, "task-manager", &self.theme)?;
        doc.color_variable("--accent", UiColor::from_rgb(accent, 255))?;
        doc.color_variable("--accent-soft", UiColor::from_rgb(accent, 42))?;
        // Text and overlay tokens follow the appearance; the root defaults in task-manager.css are Dark.
        flamewm_ui_document::appearance_tokens::project_appearance_tokens(doc, &self.theme)?;
        doc.color_variable(
            "--border",
            flamewm_ui_x11::surface_style::surface_outline(&self.theme),
        )?;
        let menu = menu_surface(&self.theme);
        for id in ["context", "confirm"] {
            doc.background(
                id,
                UiColor::from_rgb(menu.fill.rgb, menu.fill.alpha.max(242)),
            )?;
            doc.corner_radius(id, menu.radius as f32)?;
        }
        doc.foreground("menu-end-text", UiColor::from_rgb(DANGER_RGB, 255))
    }
    fn paint_metrics(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let snapshot = &self.snapshot;
        let count = snapshot.cores.len();
        let columns = count.div_ceil(CORES_PER_COLUMN).clamp(1, CORE_COLUMNS);
        let track = core_track_width(self.window.0, columns);
        // Two buffers reused for every element id and text cell: the metrics card updates on each
        // sample, and `text_ref` then skips unchanged cells instead of allocating ~80 strings.
        let mut id = String::with_capacity(24);
        let mut text = String::with_capacity(64);
        for column in 0..CORE_COLUMNS {
            id.clear();
            let _ = write!(id, "core-col-{column}");
            doc.visible(&id, column == 0 || column * CORES_PER_COLUMN < count)?;
        }
        for i in 0..CORES {
            let visible = i < count;
            id.clear();
            let _ = write!(id, "core-{i}");
            doc.visible(&id, visible)?;
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
            id.clear();
            let _ = write!(id, "core-label-{i}");
            text.clear();
            if overflow {
                let _ = write!(text, "{i}+");
            } else {
                let _ = write!(text, "{i}");
            }
            doc.text_ref(&id, &text)?;
            id.clear();
            let _ = write!(id, "core-pct-{i}");
            text.clear();
            let _ = write!(text, "{value:.0}%");
            doc.text_ref(&id, &text)?;
            id.clear();
            let _ = write!(id, "cpu-fill-{i}");
            doc.size(&id, track * value as f32 / 100.0, 7.0)?;
            doc.background(&id, UiColor::from_rgb(load_color(value), 255))?;
        }
        text.clear();
        let _ = write!(text, "{count} threads");
        doc.text_ref("cores-count", &text)?;
        text.clear();
        let _ = write!(text, "{:.0}%", snapshot.cpu_total);
        doc.text_ref("cpu-total", &text)?;
        doc.text_ref("cpu-value", &text)?;
        text.clear();
        let _ = write!(text, "Load {}", snapshot.load);
        doc.text_ref("cpu-foot", &text)?;
        text.clear();
        let _ = write!(
            text,
            "{:.1} / {:.1} GiB",
            gib(snapshot.mem_used),
            gib(snapshot.mem_total)
        );
        doc.text_ref("mem-value", &text)?;
        text.clear();
        let _ = write!(
            text,
            "{:.1} / {:.1} GiB",
            gib(snapshot.swap_used),
            gib(snapshot.swap_total)
        );
        doc.text_ref("swap-text", &text)?;
        doc.size(
            "swap-fill",
            fraction_width(snapshot.swap_used, snapshot.swap_total, 60),
            5.0,
        )?;
        rate_into(&mut text, snapshot.net_rx + snapshot.net_tx);
        doc.text_ref("net-value", &text)?;
        rate_into(&mut text, snapshot.net_rx);
        doc.text_ref("net-down", &text)?;
        rate_into(&mut text, snapshot.net_tx);
        doc.text_ref("net-up", &text)?;
        text.clear();
        let _ = write!(
            text,
            "{} processes · {} threads · {} running · up {}",
            snapshot.tasks, snapshot.threads, snapshot.running, snapshot.uptime
        );
        doc.text_ref("summary", &text)?;
        text.clear();
        let _ = write!(text, "{:.0}%", snapshot.cpu_total);
        doc.text_ref("head-cpu-pct", &text)?;
        text.clear();
        let _ = write!(
            text,
            "{:.0}%",
            fraction_width(snapshot.mem_used, snapshot.mem_total, 100)
        );
        doc.text_ref("head-mem-pct", &text)
    }
    fn paint_graphs(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let network_scale = self
            .histories
            .download
            .peak()
            .max(self.histories.upload.peak())
            .max(MIN_NETWORK_SCALE);
        // Alternate the two raster buffers so the one the document still holds from the previous
        // sample is never the one being overwritten: `Arc::make_mut` then reuses it in place.
        self.graph_phase ^= 1;
        let phase = self.graph_phase;
        {
            let histories = &self.histories;
            let cpu = Arc::make_mut(&mut self.graph_rasters[0][phase]);
            graph::render_into(
                &[graph::Series {
                    history: &histories.cpu,
                    rgb: CPU_RGB,
                    filled: true,
                }],
                100.0,
                cpu,
            );
            let memory = Arc::make_mut(&mut self.graph_rasters[1][phase]);
            graph::render_into(
                &[graph::Series {
                    history: &histories.memory,
                    rgb: MEMORY_RGB,
                    filled: true,
                }],
                100.0,
                memory,
            );
            let net = Arc::make_mut(&mut self.graph_rasters[2][phase]);
            graph::render_into(
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
                net,
            );
        }
        let rasters = [
            Arc::clone(&self.graph_rasters[0][phase]),
            Arc::clone(&self.graph_rasters[1][phase]),
            Arc::clone(&self.graph_rasters[2][phase]),
        ];
        for (id, pixels) in ["cpu-graph", "mem-graph", "net-graph"]
            .into_iter()
            .zip(rasters)
        {
            doc.image_rgba8_shared(id, id.to_owned(), graph::WIDTH, graph::HEIGHT, pixels)?;
        }
        Ok(())
    }
    fn paint_table(&mut self, doc: &mut impl UiDocumentAccess, accent: u32) -> Result<(), String> {
        self.ensure_hits();
        let max_first = self.hits.len().saturating_sub(self.viewport);
        self.first = self.first.min(max_first);
        // One id and one value buffer for the whole table: a visible row costs no `String`
        // allocation per cell, and `text_ref` skips unchanged cells inside the document.
        let mut id = String::with_capacity(32);
        let mut value = String::with_capacity(96);
        value.clear();
        if self.search_active {
            let _ = write!(value, "{}▌", self.query);
        } else if self.query.is_empty() {
            value.push_str("Search processes");
        } else {
            let _ = write!(value, "{}  ×", self.query);
        }
        doc.text_ref("search", &value)?;
        doc.text_ref("status", &self.message)?;
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
            value.clear();
            let _ = write!(value, "{label}{arrow}");
            doc.text_ref(id, &value)?;
        }
        self.rows.clear();
        let total = self.snapshot.mem_total.max(1);
        for slot in 0..ROWS {
            id.clear();
            let _ = write!(id, "row-{slot}");
            let visible = slot < self.viewport && self.first + slot < self.hits.len();
            doc.visible(&id, visible)?;
            if !visible {
                continue;
            }
            let p = &self.snapshot.processes[self.hits[self.first + slot]];
            self.rows.push((p.pid, p.start));
            let indent = INDENTS[p.depth.min(8) as usize];
            cell_id(&mut id, slot, "name");
            value.clear();
            let _ = write!(value, "{indent}{}", display_name(p));
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "dot");
            doc.background(&id, UiColor::from_rgb(state_color(p.state), 255))?;
            cell_id(&mut id, slot, "pid");
            value.clear();
            let _ = write!(value, "{}", p.pid);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "user");
            doc.text_ref(&id, &p.user)?;
            cell_id(&mut id, slot, "state");
            doc.text_ref(&id, state_name(p.state))?;
            cell_id(&mut id, slot, "threads");
            value.clear();
            let _ = write!(value, "{}", p.threads);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "pri");
            value.clear();
            let _ = write!(value, "{}", p.priority);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "nice");
            value.clear();
            let _ = write!(value, "{}", p.nice);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "virt");
            memory_text_into(&mut value, p.virt_kib);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "cpu");
            value.clear();
            let _ = write!(value, "{:.1}%", p.cpu);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "mem");
            memory_text_into(&mut value, p.rss_kib);
            doc.text_ref(&id, &value)?;
            cell_id(&mut id, slot, "cmd");
            doc.text_ref(&id, &p.command)?;
            let memory_share = 100.0 * p.rss_kib as f64 / total as f64;
            cell_id(&mut id, slot, "cpu");
            doc.background(&id, UiColor::from_rgb(HEAT_RGB, heat_alpha(p.cpu / 50.0)))?;
            cell_id(&mut id, slot, "mem");
            doc.background(
                &id,
                UiColor::from_rgb(HEAT_RGB, heat_alpha(memory_share / 10.0)),
            )?;
            id.clear();
            let _ = write!(id, "row-{slot}");
            if self.selected == Some((p.pid, p.start)) {
                doc.background(&id, UiColor::from_rgb(accent, 60))?;
            } else if self.hovered == Some(slot) {
                doc.background(&id, UiColor::from_rgb(accent, 32))?;
            } else {
                doc.background_clear(&id)?;
            }
        }
        Ok(())
    }
    fn paint_menus(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        doc.visible("context", self.context)?;
        doc.visible("confirm", self.confirm)?;
        // No menu open: skip the title/label formatting and the selected-process scan entirely.
        if !self.context && !self.confirm {
            return Ok(());
        }
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
        self.hits_dirty = true;
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
                self.ensure_hits();
                let max = self.hits.len().saturating_sub(self.viewport);
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
            let next = event
                .action
                .strip_prefix("tm.row.")
                .and_then(|slot| slot.parse::<usize>().ok())
                .filter(|slot| *slot < self.rows.len());
            if next != self.hovered {
                let previous = std::mem::replace(&mut self.hovered, next);
                let accent = accent_color(&self.theme);
                for slot in [previous, next].into_iter().flatten() {
                    let id = format!("row-{slot}");
                    if let Some((pid, start)) = self.rows.get(slot).copied() {
                        if self.selected == Some((pid, start)) {
                            doc.background(&id, UiColor::from_rgb(accent, 60))?;
                        } else if next == Some(slot) {
                            doc.background(&id, UiColor::from_rgb(accent, 32))?;
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
                    self.hits_dirty = true;
                    self.first = 0;
                }
                _ => {
                    if let Some(text) = &key.text {
                        self.query
                            .extend(text.chars().filter(|c| !c.is_control()).take(8));
                        if self.query.chars().count() > 128 {
                            self.query = self.query.chars().take(128).collect();
                        }
                        self.hits_dirty = true;
                        self.first = 0;
                    }
                }
            }
        } else {
            match name.as_str() {
                "f3" | "/" => self.search_active = true,
                "escape" => {
                    self.query.clear();
                    self.hits_dirty = true;
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
fn display_name(process: &model::Process) -> &str {
    const COMM_LIMIT: usize = 15;
    if process.name.len() < COMM_LIMIT {
        return process.name.as_ref();
    }
    process
        .command
        .split_whitespace()
        .next()
        .and_then(|program| program.rsplit('/').next())
        .filter(|program| program.starts_with(process.name.as_ref()))
        .unwrap_or(process.name.as_ref())
}
/// Writes one row-cell element id into the reused buffer: no `format!` per cell per paint.
fn cell_id(id: &mut String, slot: usize, cell: &str) {
    id.clear();
    let _ = write!(id, "r-{slot}-{cell}");
}
/// Formats a memory size into the reused buffer; `text_ref` then skips unchanged cells.
fn memory_text_into(out: &mut String, kib: u64) {
    out.clear();
    if kib >= 1024 * 1024 {
        let _ = write!(out, "{:.2} GB", kib as f64 / 1_048_576.0);
    } else {
        let _ = write!(out, "{:.1} MB", kib as f64 / 1024.0);
    }
}
fn rate_into(out: &mut String, bytes_per_second: f64) {
    out.clear();
    let value = bytes_per_second.max(0.0);
    if value >= 1_048_576.0 {
        let _ = write!(out, "{:.1} MiB/s", value / 1_048_576.0);
    } else if value >= 1024.0 {
        let _ = write!(out, "{:.1} KiB/s", value / 1024.0);
    } else {
        let _ = write!(out, "{value:.0} B/s");
    }
}
/// Substring test that ignores ASCII case, where `needle` is already lowercased. The search runs
/// on every process on every paint; doing it on bytes avoids the two `String` allocations that
/// `to_lowercase()` made per process. Non-ASCII text keeps the exact former behaviour.
fn contains_folded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
        return haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle));
    }
    haystack.to_lowercase().contains(needle)
}
/// Matches a PID against `needle` by writing its decimal digits into a stack buffer, so the
/// search never allocates the `to_string()` of every process.
fn pid_contains(pid: u32, needle: &str) -> bool {
    let mut digits = [0_u8; 10];
    let mut start = digits.len();
    let mut value = pid;
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    std::str::from_utf8(&digits[start..])
        .unwrap_or_default()
        .contains(needle)
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
fn accent_color(snapshot: &SettingsSnapshot) -> u32 {
    flamewm_ui_core::accent::accent(snapshot).0
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
        _ => flamewm_api::settings_groups::DEFAULT_SURFACE_OPACITY,
    };
    let fit = text("wallpaperFit");
    let (opacity_percent, blur_percent) =
        flamewm_ui_core::glass::window_backdrop_material(snapshot, opacity, blur);
    flamewm_ui_x11::backdrop::WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: flamewm_api::paths::workspace_packaged_root(),
        opacity_percent,
        blur_percent,
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
    let gate = Arc::new(sample_gate::SampleGate::default());
    let worker_gate = Arc::clone(&gate);
    // One sampler exists only while this window is open; never a permanent desktop daemon.
    let worker = std::thread::spawn(move || {
        let mut sampler = model::Sampler::default();
        while active.load(Ordering::Relaxed) {
            // Obscured: sleep with no timer until uncovered. Visible: the loop below is unchanged.
            if !worker_gate.wait_visible(|| active.load(Ordering::Relaxed)) {
                break;
            }
            let snapshot = sampler.collect();
            if sender.try_send(snapshot).is_ok() {
                match writer.write(&[1]) {
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(_) => break,
                }
            }
            // Sleep on the action channel until the next sample is due: no 100 ms polling timer.
            // A queued process operation wakes the worker at once; the sample cadence stays 1 s.
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            loop {
                if !active.load(Ordering::Relaxed) {
                    break;
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match action_rx.recv_timeout(remaining) {
                    // Process operations may block, but only on this sampler worker.
                    // The caller-selected PID/start-time pair is checked by the operation.
                    Ok(command) => {
                        let _ = result_tx.try_send(command.execute());
                        let _ = writer.write(&[1]);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(remaining);
                        break;
                    }
                }
            }
        }
    });
    let queue = Rc::new(RefCell::new(receiver));
    let window = Rc::new(RefCell::new(app));
    let event_window = Rc::clone(&window);
    let visibility_gate = Arc::clone(&gate);
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
                    app.metrics_dirty = true;
                    app.paint(doc)
                }
                UiControllerEvent::Drag(_) => Ok(()),
                UiControllerEvent::Visibility { obscured } => {
                    visibility_gate.set_visible(!*obscured);
                    Ok(())
                }
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
    gate.wake();
    let _ = worker.join();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_sort_is_case_insensitive_with_pid_tiebreak() {
        let proc = |pid: u32, name: &str| {
            let mut p = model::Process::default();
            p.pid = pid;
            p.name = name.into();
            p
        };
        let (actions, _rx) = std::sync::mpsc::sync_channel(1);
        let mut app = Application::new(SettingsSnapshot::new(0), actions);
        app.snapshot.processes = vec![
            proc(3, "bash"),
            proc(1, "Zed"),
            proc(2, "apple"),
            proc(4, "Apple"),
        ];
        app.sort = Sort::Name;
        app.descending = false;
        let names: Vec<u32> = app
            .sorted()
            .into_iter()
            .map(|i| app.snapshot.processes[i].pid)
            .collect();
        assert_eq!(names, vec![2, 4, 3, 1]);
    }
    #[test]
    fn memory_progress_is_bounded_in_all_cases() {
        assert_eq!(fraction_width(200, 100, 240), 240.0);
        assert_eq!(fraction_width(0, 0, 240), 0.0);
        assert_eq!(fraction_width(50, 100, 240), 120.0);
    }
    #[test]
    fn default_accent_and_surface_match_flamewm_settings() {
        let snap = SettingsSnapshot::new(0);
        assert_eq!(accent_color(&snap), 0x003d_aee9);
        // Translucent default: the surface is 75% alpha.
        assert_eq!(surface_fill(&snap).alpha, 191);
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
        let rate = |bytes| {
            let mut text = String::new();
            rate_into(&mut text, bytes);
            text
        };
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
        assert_eq!(rate(3.0 * 1_048_576.0), "3.0 MiB/s");
        let memory = |kib| {
            let mut text = String::new();
            memory_text_into(&mut text, kib);
            text
        };
        assert_eq!(memory(2048), "2.0 MB");
        assert_eq!(memory(3 * 1024 * 1024), "3.00 GB");
    }
    #[test]
    fn default_rows_fit_without_clipping_and_resize_is_bounded() {
        assert_eq!(visible_rows(HEIGHT), 15);
        assert_eq!(visible_rows(0), 1);
        assert_eq!(visible_rows(4096), ROWS);
    }
    #[test]
    fn search_folds_ascii_case_and_matches_pid_digits_in_place() {
        assert!(contains_folded("Firefox", "fire"));
        assert!(contains_folded("firefox", "FIRE"));
        assert!(!contains_folded("firefox", "chrome"));
        assert!(!contains_folded("fire", "firefox"));
        assert!(pid_contains(4242, "242"));
        assert!(pid_contains(0, "0"));
        assert!(!pid_contains(4242, "999"));
    }
}
