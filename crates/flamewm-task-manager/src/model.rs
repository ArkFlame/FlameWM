//! Linux procfs sampler. All filesystem reads run off the X11/UI thread.
//! PID plus process start-time is the authority for selecting a live process.
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct Process {
    pub pid: u32,
    pub start: u64,
    pub name: String,
    pub uid: u32,
    pub user: String,
    pub state: char,
    pub nice: i32,
    pub priority: i64,
    pub threads: u32,
    pub rss_kib: u64,
    pub virt_kib: u64,
    pub cpu: f64,
    pub command: String,
    ticks: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub processes: Vec<Process>,
    pub cores: Vec<f64>,
    pub cpu_total: f64,
    pub mem_total: u64,
    pub mem_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub load: String,
    pub uptime: String,
    pub tasks: usize,
    pub threads: u64,
    pub running: usize,
    pub net_rx: f64,
    pub net_tx: f64,
}

#[derive(Default)]
pub struct Sampler {
    prev_global: Option<Cpu>,
    prev_cores: Vec<Cpu>,
    /// Per-PID state kept between samples; entries not seen in the last sample are dropped.
    tasks: HashMap<u32, Cached>,
    /// Reused read buffer for every procfs file of a sample.
    scratch: Vec<u8>,
    tick: u64,
    page_bytes: u64,
    user_cache: Option<HashMap<u32, String>>,
    prev_net: Option<(u64, u64, Instant)>,
}

/// Static-per-process fields are re-read on first sight, on a rename (comm changed), and at
/// most every `STATIC_REFRESH_TICKS` samples, which catches same-name exec and setuid.
const STATIC_REFRESH_TICKS: u64 = 5;

#[derive(Default)]
struct Cached {
    start: u64,
    ticks: u64,
    seen: u64,
    refreshed: u64,
    name: String,
    uid: u32,
    user: String,
    command: String,
}

#[derive(Clone, Copy, Default)]
struct Cpu {
    total: u64,
    busy: u64,
}
fn cpu_ticks(line: &str) -> Option<Cpu> {
    let mut fields = line.split_whitespace();
    let _name = fields.next()?;
    let values: Vec<u64> = fields
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values.iter().sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(Cpu {
        total,
        busy: total - idle,
    })
}
fn load_cpu() -> (Cpu, Vec<Cpu>) {
    let text = fs::read_to_string("/proc/stat").unwrap_or_default();
    let mut lines = text.lines().take_while(|line| line.starts_with("cpu"));
    let total = lines.next().and_then(cpu_ticks).unwrap_or_default();
    (total, lines.filter_map(cpu_ticks).collect())
}
fn percent(now: Cpu, old: Cpu) -> f64 {
    let total = now.total.saturating_sub(old.total);
    if total == 0 {
        0.0
    } else {
        100.0 * now.busy.saturating_sub(old.busy) as f64 / total as f64
    }
}

/// `/proc/[pid]/stat`: the comm field may contain spaces and closing parentheses.
/// Only the final closing parenthesis can delimit it.
pub fn parse_stat(pid: u32, text: &str, page_bytes: u64) -> Option<Process> {
    let first = text.find('(')?;
    let last = text.rfind(')')?;
    if first >= last {
        return None;
    }
    let fields: Vec<&str> = text.get(last + 1..)?.split_whitespace().collect();
    if fields.len() < 22 {
        return None;
    }
    let utime = fields[11].parse::<u64>().ok()?;
    let stime = fields[12].parse::<u64>().ok()?;
    // Field 24 (rss) is in pages: it replaces /proc/<pid>/status VmRSS, one read fewer per task.
    let pages = fields[21].parse::<u64>().ok()?;
    Some(Process {
        pid,
        name: sanitize(text.get(first + 1..last)?, 64),
        state: fields[0].chars().next()?,
        priority: fields[15].parse().ok()?,
        nice: fields[16].parse().ok()?,
        threads: fields[17].parse().ok()?,
        start: fields[19].parse().ok()?,
        virt_kib: fields[20].parse::<u64>().ok()? / 1024,
        rss_kib: pages.saturating_mul(page_bytes) / 1024,
        uid: u32::MAX,
        ticks: utime.saturating_add(stime),
        ..Process::default()
    })
}
/// Page size from the auxiliary vector (AT_PAGESZ): no libc call and no extra crate.
fn page_size_bytes() -> u64 {
    const AT_PAGESZ: u64 = 6;
    let Ok(auxv) = fs::read("/proc/self/auxv") else {
        return 4096;
    };
    auxv.chunks_exact(16)
        .find_map(|pair| {
            let key = u64::from_ne_bytes(pair[..8].try_into().ok()?);
            let value = u64::from_ne_bytes(pair[8..].try_into().ok()?);
            (key == AT_PAGESZ && value != 0).then_some(value)
        })
        .unwrap_or(4096)
}
/// Reads a procfs file into the reused `buf`. `None` when the file is gone or unreadable.
fn read_proc<'a>(buf: &'a mut Vec<u8>, path: &str) -> Option<&'a [u8]> {
    use std::io::Read;
    buf.clear();
    let mut file = fs::File::open(path).ok()?;
    let mut chunk = [0_u8; 2048];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(buf.as_slice())
}
fn sanitize(text: &str, max_chars: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max_chars)
        .collect()
}
/// Real UID (first `Uid:` field) of `/proc/<pid>/status`; `u32::MAX` when unknown.
pub fn parse_uid(status: &str) -> u32 {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(u32::MAX)
}
fn command(scratch: &mut Vec<u8>, pid: u32, name: &str) -> String {
    if let Some(bytes) = read_proc(scratch, &format!("/proc/{pid}/cmdline")) {
        let args = String::from_utf8_lossy(bytes);
        let text = sanitize(&args.replace('\0', " "), 180).trim().to_owned();
        if !text.is_empty() {
            return text;
        }
    }
    format!("[{name}]")
}
fn memory() -> (u64, u64, u64, u64) {
    let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut fields = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if let Some(kib) = value
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        {
            fields.insert(key, kib);
        }
    }
    let get = |key| fields.get(key).copied().unwrap_or(0_u64);
    let total = get("MemTotal");
    let available = get("MemAvailable");
    let swap_total = get("SwapTotal");
    (
        total,
        total.saturating_sub(available),
        swap_total,
        swap_total.saturating_sub(get("SwapFree")),
    )
}
/// Received and transmitted byte totals of every interface but loopback in `/proc/net/dev`.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    text.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            if name.trim() == "lo" {
                return None;
            }
            let mut fields = counters.split_whitespace();
            let rx = fields.next()?.parse::<u64>().ok()?;
            let tx = fields.nth(7)?.parse::<u64>().ok()?;
            Some((rx, tx))
        })
        .fold((0, 0), |(rx, tx), (r, t)| {
            (rx.saturating_add(r), tx.saturating_add(t))
        })
}
fn user_names() -> HashMap<u32, String> {
    // /etc/passwd is read once by the worker, not on each frame or in the UI callback.
    fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?.to_owned();
            let _password = fields.next()?;
            let uid = fields.next()?.parse::<u32>().ok()?;
            Some((uid, name))
        })
        .collect()
}
impl Sampler {
    pub fn collect(&mut self) -> Snapshot {
        if self.user_cache.is_none() {
            self.user_cache = Some(user_names());
        }
        let (now, cores) = load_cpu();
        let (mem_total, mem_used, swap_total, swap_used) = memory();
        let mut result = Snapshot {
            mem_total,
            mem_used,
            swap_total,
            swap_used,
            load: fs::read_to_string("/proc/loadavg")
                .unwrap_or_default()
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" "),
            uptime: fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
                .map_or_else(
                    || "Unavailable".into(),
                    |secs| {
                        let secs = secs as u64;
                        format!(
                            "{}d {:02}:{:02}:{:02}",
                            secs / 86400,
                            secs / 3600 % 24,
                            secs / 60 % 60,
                            secs % 60
                        )
                    },
                ),
            cpu_total: self
                .prev_global
                .map_or(0.0, |previous| percent(now, previous)),
            cores: cores
                .iter()
                .enumerate()
                .map(|(i, cpu)| {
                    self.prev_cores
                        .get(i)
                        .map_or(0.0, |old| percent(*cpu, *old))
                })
                .collect(),
            ..Snapshot::default()
        };
        let delta = self
            .prev_global
            .map_or(0, |old| now.total.saturating_sub(old.total));
        let cpu_count = cores.len().max(1) as f64;
        if self.page_bytes == 0 {
            self.page_bytes = page_size_bytes();
        }
        self.tick += 1;
        let tick = self.tick;
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                let Some(task) = self.sample_task(pid, tick, delta, cpu_count) else {
                    continue;
                };
                result.threads += u64::from(task.threads);
                if task.state == 'R' {
                    result.running += 1;
                }
                result.processes.push(task);
            }
        }
        self.tasks.retain(|_, cached| cached.seen == tick);
        result.tasks = result.processes.len();
        let (rx, tx) = parse_net_dev(&fs::read_to_string("/proc/net/dev").unwrap_or_default());
        let sampled = Instant::now();
        if let Some((prev_rx, prev_tx, at)) = self.prev_net {
            let seconds = sampled.duration_since(at).as_secs_f64();
            if seconds > 0.0 {
                result.net_rx = rx.saturating_sub(prev_rx) as f64 / seconds;
                result.net_tx = tx.saturating_sub(prev_tx) as f64 / seconds;
            }
        }
        self.prev_net = Some((rx, tx, sampled));
        self.prev_global = Some(now);
        self.prev_cores = cores;
        result
    }

    /// One task per sample: `stat` every time (CPU and RSS), `status` and `cmdline` only when
    /// the cached static fields are stale. Returns `None` for tasks that vanished mid-read.
    fn sample_task(&mut self, pid: u32, tick: u64, delta: u64, cpu_count: f64) -> Option<Process> {
        let text = read_proc(&mut self.scratch, &format!("/proc/{pid}/stat"))?;
        let mut task = parse_stat(pid, std::str::from_utf8(text).ok()?, self.page_bytes)?;
        let cached = self.tasks.entry(pid).or_default();
        // Same PID and start time as the previous sample: the delta is meaningful. PID reuse
        // changes the start time, so a recycled PID never inherits the old CPU counter.
        let known = cached.seen != 0 && cached.start == task.start;
        if known && delta != 0 {
            task.cpu =
                100.0 * cpu_count * task.ticks.saturating_sub(cached.ticks) as f64 / delta as f64;
        }
        let stale = !known
            || cached.name != task.name
            || tick.saturating_sub(cached.refreshed) >= STATIC_REFRESH_TICKS;
        if stale {
            let status = read_proc(&mut self.scratch, &format!("/proc/{pid}/status"));
            cached.uid = status
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map_or(u32::MAX, parse_uid);
            cached.user = self
                .user_cache
                .as_ref()
                .and_then(|names| names.get(&cached.uid))
                .cloned()
                .unwrap_or_else(|| {
                    if cached.uid == u32::MAX {
                        "?".into()
                    } else {
                        cached.uid.to_string()
                    }
                });
            // exec can replace argv without changing PID/starttime; the refresh bounds that lag.
            cached.command = command(&mut self.scratch, pid, &task.name);
            cached.refreshed = tick;
        }
        if cached.name != task.name {
            cached.name.clone_from(&task.name);
        }
        cached.start = task.start;
        cached.ticks = task.ticks;
        cached.seen = tick;
        task.uid = cached.uid;
        task.user.clone_from(&cached.user);
        task.command.clone_from(&cached.command);
        Some(task)
    }
}

/// Refuse PID reuse between display and process operation. No shell expansion is possible.
pub fn check_identity(pid: u32, start: u64) -> bool {
    let path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(path) else {
        return false;
    };
    parse_stat(pid, &stat, 0).is_some_and(|current| current.start == start)
}

pub fn change_nice(pid: u32, start: u64, nice: i32) -> Result<(), String> {
    if !(-20..=19).contains(&nice) || !check_identity(pid, start) {
        return Err("Process exited, PID changed, or priority is invalid".into());
    }
    // `renice` is executed directly with numeric argv, never through a shell or sudo.
    let output = std::process::Command::new("renice")
        .args(["-n", &nice.to_string(), "-p", &pid.to_string()])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(sanitize(&String::from_utf8_lossy(&output.stderr), 180))
    }
}

pub fn terminate(pid: u32, start: u64) -> Result<(), String> {
    if pid <= 1 || !check_identity(pid, start) {
        return Err("Process exited or PID changed".into());
    }
    // Requires an explicit confirmation by the user and is subject to normal OS permissions.
    let output = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(sanitize(&String::from_utf8_lossy(&output.stderr), 180))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_stat_handles_spaces_and_closing_paren_in_command() {
        let mut fields = vec!["S".to_string(); 22];
        fields[11] = "120".into();
        fields[12] = "30".into();
        fields[15] = "25".into();
        fields[16] = "5".into();
        fields[17] = "2".into();
        fields[19] = "999999".into();
        fields[20] = "1048576".into();
        fields[21] = "512".into();
        let line = format!("23 (some ) process) {}", fields.join(" "));
        let p = parse_stat(23, &line, 4096).expect("parse");
        assert_eq!(p.name, "some ) process");
        assert_eq!(
            (
                p.start, p.priority, p.nice, p.threads, p.virt_kib, p.rss_kib
            ),
            (999_999, 25, 5, 2, 1024, 2048)
        );
    }
    /// The pre-change parser: `VmRSS` and `Uid` read from `/proc/<pid>/status`. Kept as the reference.
    fn legacy_status(text: &str) -> (u32, u64) {
        let (mut uid, mut rss) = (u32::MAX, 0);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("Uid:") {
                uid = v
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(u32::MAX);
            } else if let Some(v) = line.strip_prefix("VmRSS:") {
                rss = v
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
        (uid, rss)
    }
    #[test]
    fn stat_rss_and_status_uid_match_the_legacy_parser() {
        // Fixture /proc/<pid>/stat and /proc/<pid>/status (RSS 1030 pages, VSZ 6291456 bytes).
        let stat = "4242 (fire fox) S 1 4242 4242 0 -1 4194560 1 0 0 0 150 30 0 0 20 0 7 0 555 6291456 1030 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let status = "Name:\tfire fox\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t    4120 kB\n";
        let task = parse_stat(4242, stat, 4096).expect("parse");
        let (uid, rss) = legacy_status(status);
        assert_eq!((task.rss_kib, parse_uid(status)), (rss, uid));
        assert_eq!(
            (
                uid,
                task.name.as_str(),
                task.start,
                task.virt_kib,
                task.threads
            ),
            (1000, "fire fox", 555, 6144, 7)
        );
        assert_eq!(parse_stat(4242, stat, 16384).expect("parse").rss_kib, 16480);
        // A task that vanished between the two reads keeps the legacy "unknown" defaults.
        assert_eq!(parse_uid(""), legacy_status("").0);
        assert_eq!(parse_uid(""), u32::MAX);
    }
    #[test]
    fn sampling_never_counts_process_reuse_as_cpu_usage() {
        let old = (31_u64, 100_u64);
        let new = (32_u64, 200_u64);
        assert_ne!(old.0, new.0);
        assert_eq!(
            percent(
                Cpu {
                    total: 100,
                    busy: 50
                },
                Cpu {
                    total: 100,
                    busy: 50
                }
            ),
            0.0
        );
    }
    #[test]
    fn net_dev_totals_skip_loopback_and_header() {
        let text = "Inter-|   Receive |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes\n    lo: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n  eth0: 1000 10 0 0 0 0 0 0 250 3 0 0 0 0 0 0\n wlan0: 24 1 0 0 0 0 0 0 6 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(text), (1024, 256));
        assert_eq!(parse_net_dev(""), (0, 0));
    }
    #[test]
    fn priorities_are_bounded_without_unsigned_overflow() {
        assert!(change_nice(0, 0, -21).is_err());
        assert!(change_nice(0, 0, 20).is_err());
        assert!(!check_identity(u32::MAX, 0));
    }
    #[test]
    fn can_sample_procfs_without_panicking_on_exiting_tasks() {
        let _ = Sampler::default().collect();
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    /// utime + stime of this process in clock ticks (CLK_TCK is 100 on Linux targets FlameWM ships for).
    fn self_cpu_ticks() -> u64 {
        let text = fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let fields: Vec<&str> = text
            .rsplit_once(')')
            .map(|(_, rest)| rest.split_whitespace().collect())
            .unwrap_or_default();
        match (fields.get(11), fields.get(12)) {
            (Some(u), Some(s)) => u.parse::<u64>().unwrap_or(0) + s.parse::<u64>().unwrap_or(0),
            _ => 0,
        }
    }
    /// cargo test -p flamewm-task-manager --release bench_sampler -- --ignored --nocapture
    #[test]
    #[ignore = "measurement only"]
    fn bench_sampler_cpu_per_sample() {
        const SAMPLES: u32 = 600;
        let mut sampler = Sampler::default();
        let _ = std::hint::black_box(sampler.collect());
        let before = self_cpu_ticks();
        let started = Instant::now();
        let mut processes = 0;
        for _ in 0..SAMPLES {
            let snapshot = sampler.collect();
            processes = snapshot.processes.len();
            std::hint::black_box(&snapshot);
        }
        let wall = started.elapsed().as_secs_f64() * 1000.0 / f64::from(SAMPLES);
        let cpu = self_cpu_ticks().saturating_sub(before) as f64 * 10.0 / f64::from(SAMPLES);
        println!(
            "bench samples={SAMPLES} processes={processes} cpu_ms_per_sample={cpu:.3} wall_ms_per_sample={wall:.3}"
        );
    }
}
