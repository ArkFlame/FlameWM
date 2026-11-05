use super::*;

#[derive(Debug, Default)]
pub(super) struct Samples {
    pub(super) cpu_previous: Option<CpuTimes>,
    pub(super) cpu_percent: Option<u8>,
    pub(super) cpu_count: usize,
    pub(super) cpu_history: VecDeque<u8>,
    pub(super) memory: Option<MemoryUsage>,
    pub(super) memory_history: VecDeque<u8>,
    pub(super) uptime_seconds: Option<u64>,
    pub(super) net_previous: Option<(u64, u64, Instant)>,
    pub(super) net_rx_history: VecDeque<u64>,
    pub(super) net_tx_history: VecDeque<u64>,
}

/// Sums non-loopback received and transmitted bytes from `/proc/net/dev`.
pub(super) fn parse_net_dev(text: &str) -> Option<(u64, u64)> {
    let mut totals = None;
    for line in text.lines().skip(2) {
        let Some((name, fields)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<u64> = fields
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        if let (Some(rx), Some(tx)) = (fields.first(), fields.get(8)) {
            let (sum_rx, sum_tx) = totals.unwrap_or((0u64, 0u64));
            totals = Some((sum_rx + rx, sum_tx + tx));
        }
    }
    totals
}

pub(super) fn push_rate(history: &mut VecDeque<u64>, value: u64) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

/// Scales rates to percentages of the largest sample in view, so the graph autoscales.
pub(super) fn scale_rates(history: &VecDeque<u64>) -> VecDeque<u8> {
    let peak = history.iter().copied().max().unwrap_or(0).max(1);
    history
        .iter()
        .map(|rate| (rate * 100 / peak) as u8)
        .collect()
}

pub(super) fn push_history(history: &mut VecDeque<u8>, value: Option<u8>) {
    if let Some(value) = value {
        if history.len() == HISTORY_LEN {
            history.pop_front();
        }
        history.push_back(value);
    }
}

impl Samples {
    /// Pushes download and upload byte rates since the previous refresh.
    pub(super) fn refresh_network(&mut self, now: Instant) {
        let Some(totals) = fs::read_to_string("/proc/net/dev")
            .ok()
            .and_then(|text| parse_net_dev(&text))
        else {
            return;
        };
        if let Some((rx, tx, at)) = self.net_previous {
            let seconds = now.duration_since(at).as_secs_f64().max(0.001);
            let rate = |now: u64, before: u64| {
                (now.saturating_sub(before) as f64 / seconds).round() as u64
            };
            push_rate(&mut self.net_rx_history, rate(totals.0, rx));
            push_rate(&mut self.net_tx_history, rate(totals.1, tx));
        }
        self.net_previous = Some((totals.0, totals.1, now));
    }

    pub(super) fn refresh(&mut self, cpu: bool, memory: bool, network: bool, uptime: bool) {
        if cpu && let Ok(stat) = fs::read_to_string("/proc/stat") {
            let current = parse_cpu_times(&stat);
            if let (Some(previous), Some(current)) = (self.cpu_previous, current) {
                self.cpu_percent = cpu_percent(previous, current).or(self.cpu_percent);
            }
            self.cpu_previous = current.or(self.cpu_previous);
            self.cpu_count = count_cpus(&stat);
            push_history(&mut self.cpu_history, self.cpu_percent);
        }
        if memory {
            self.memory = fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| parse_meminfo(&text));
            push_history(
                &mut self.memory_history,
                self.memory.and_then(MemoryUsage::percent),
            );
        }
        if network {
            self.refresh_network(Instant::now());
        }
        if uptime {
            self.uptime_seconds = fs::read_to_string("/proc/uptime").ok().and_then(|text| {
                text.split_whitespace()
                    .next()?
                    .parse::<f64>()
                    .ok()
                    .map(|seconds| seconds as u64)
            });
        }
    }
}
