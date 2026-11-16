use super::*;

impl WidgetState {
    pub(crate) fn new(store: WidgetStore, path: PathBuf) -> Self {
        Self {
            store,
            path,
            gesture: None,
            slot_overlay: false,
            editing: None,
            samples: Samples::default(),
            media: MediaSnapshot::default(),
            mpris: None,
            media_dirty: false,
            wake: WakeDeadline::default(),
            due: None,
            sample_due: None,
            sync_key: None,
            obscured: false,
            pending_pointer: None,
            rings: [None; MAX_WIDGETS],
            graphs: std::array::from_fn(|_| None),
            faces: [None; MAX_WIDGETS],
            username: std::env::var("USER")
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "friend".to_owned()),
            hostname: fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|name| name.trim().to_owned())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn attach_wake(&mut self, wake: WakeDeadline) {
        self.wake = wake;
        self.arm(Instant::now());
    }

    pub(super) fn tick_rate(&self) -> Option<TickRate> {
        let widgets = self.store.widgets();
        if widgets.iter().any(|widget| {
            widget.kind == WidgetKind::Custom
                || (widget.kind == WidgetKind::Clock
                    && (widget.flag("showSeconds") || is_analog(widget)))
        }) {
            Some(TickRate::Second)
        } else if widgets
            .iter()
            .any(|widget| matches!(widget.kind, WidgetKind::Clock | WidgetKind::Calendar))
        {
            Some(TickRate::Minute)
        } else {
            None
        }
    }

    pub(super) fn arm(&mut self, now: Instant) {
        let sampling = self.store.widgets().iter().any(|widget| {
            matches!(
                widget.kind,
                WidgetKind::Cpu
                    | WidgetKind::Memory
                    | WidgetKind::Ring
                    | WidgetKind::History
                    | WidgetKind::Network
                    | WidgetKind::Custom
            )
        });
        self.sample_due = sampling.then(|| self.sample_due.unwrap_or(now));
        self.due = self.tick_rate().map(|rate| {
            let epoch_ms = (flamewm_api::clock::unix_nanos() / 1_000_000) as u64;
            now + Duration::from_millis(
                wall_clock_delay_ms(epoch_ms, rate == TickRate::Second).max(1),
            )
        });
        self.wake
            .set([self.due, self.sample_due].into_iter().flatten().min());
    }

    pub(super) fn sample(&mut self) {
        let widgets = self.store.widgets();
        let custom = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Custom);
        let network = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Network);
        let cpu = custom
            || widgets.iter().any(|widget| {
                matches!(
                    widget.kind,
                    WidgetKind::Cpu | WidgetKind::History | WidgetKind::Ring
                )
            });
        let memory = custom
            || widgets.iter().any(|widget| {
                matches!(
                    widget.kind,
                    WidgetKind::Memory | WidgetKind::History | WidgetKind::Ring
                )
            });
        self.samples.refresh(cpu, memory, network, custom);
        self.sample_due = Some(Instant::now() + SAMPLE_INTERVAL);
    }

    pub(super) fn sample_key(&self) -> SampleKey {
        let samples = &self.samples;
        SampleKey {
            cpu_percent: samples.cpu_percent,
            cpu_count: samples.cpu_count,
            memory: samples.memory.map(|memory| {
                (
                    memory.percent(),
                    format_gib(memory.used_kib()),
                    format_gib(memory.total_kib),
                )
            }),
            uptime_seconds: samples.uptime_seconds,
            cpu_history: samples.cpu_history.clone(),
            memory_history: samples.memory_history.clone(),
            net_rx_history: scale_rates(&samples.net_rx_history),
            net_tx_history: scale_rates(&samples.net_tx_history),
        }
    }

    pub(crate) fn persist(&self) {
        if let Err(error) = flamewm_widgets_core::widgets::persist(&self.path, &self.store) {
            super::super::log_limit::emit(
                "widget state save failed",
                format_args!("desktop: widget state save failed: {error:?}"),
            );
        }
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub(super) fn has_media_widget(&self) -> bool {
        self.store
            .widgets()
            .iter()
            .any(|widget| widget.kind == WidgetKind::Media)
    }

    pub(super) fn set_media(&mut self, snapshot: MediaSnapshot) {
        if self.media != snapshot {
            self.media = snapshot;
            self.media_dirty |= self.has_media_widget();
        }
    }
}
