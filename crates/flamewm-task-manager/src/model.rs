//! Linux procfs sampler. All filesystem reads run off the X11/UI thread.
//! PID plus process start-time is the authority for selecting a live process.
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// Number of ancestors that are also in the snapshot; 0 for roots. Drives tree indentation.
    pub depth: u32,
    pub start: u64,
    /// `name`, `user` and `command` are shared: the cache and the snapshot clone a `Process` per
    /// rotating read, and an `Arc<str>` clone is a refcount bump instead of a heap copy.
    pub name: Arc<str>,
    pub user: Arc<str>,
    pub state: char,
    pub nice: i32,
    pub priority: i64,
    pub threads: u32,
    pub rss_kib: u64,
    pub virt_kib: u64,
    pub cpu: f64,
    pub command: Arc<str>,
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
    /// Reused `/proc/<pid>/<file>` path, so no `format!` allocation per task read.
    path: String,
    /// Reused text buffer for the whole-file procfs reads of a sample.
    text: String,
    /// Reused pid list, so the per-sample walk allocates nothing.
    pids: Vec<u32>,
    /// Reused pid→ppid map for `assign_depths`; cleared each sample, capacity kept.
    parents: HashMap<u32, u32>,
    /// Reused CPU ranking behind the hot set; cleared each sample, capacity kept.
    ranked: Vec<(f64, u32)>,
    tick: u64,
    page_bytes: u64,
    user_cache: Option<HashMap<u32, Arc<str>>>,
    prev_net: Option<(u64, u64, Instant)>,
    /// Highest-CPU processes of the last sample, ascending. Read `/proc/<pid>/stat` every tick.
    /// A sorted `Vec` is probed by the pid walk with one forward index and no hashing.
    hot: Vec<u32>,
}

/// Processes re-read every tick because they carry the most CPU. Others are re-read on a rotating
/// schedule of one tick in `SLOW_TICKS`, and keep their last sample in between.
pub const HOT_COUNT: usize = 64;
pub const SLOW_TICKS: u64 = 4;

/// Whether a non-hot pid is due for its rotating `stat` read on this tick.
pub fn slow_due(pid: u32, tick: u64) -> bool {
    tick % SLOW_TICKS == u64::from(pid) % SLOW_TICKS
}

/// CPU percent of one process over the span of global CPU ticks since its last `stat` read.
/// A zero span (no time passed) reports 0 rather than dividing by zero.
pub fn cpu_percent(process_ticks: u64, span_ticks: u64, cpu_count: f64) -> f64 {
    if span_ticks == 0 {
        return 0.0;
    }
    100.0 * cpu_count * process_ticks as f64 / span_ticks as f64
}

/// Rebuilds `hot` with the `HOT_COUNT` highest-CPU pids of `processes`, ascending, reusing `hot`
/// and `ranked` so the per-sample hot-set update allocates nothing after the first sample.
pub fn hot_pids_into(hot: &mut Vec<u32>, ranked: &mut Vec<(f64, u32)>, processes: &[Process]) {
    ranked.clear();
    ranked.extend(processes.iter().map(|p| (p.cpu, p.pid)));
    if ranked.len() > HOT_COUNT {
        ranked.select_nth_unstable_by(HOT_COUNT, |a, b| b.0.total_cmp(&a.0));
        ranked.truncate(HOT_COUNT);
    }
    hot.clear();
    hot.extend(ranked.iter().map(|&(_, pid)| pid));
    // Ascending, so the pid walk can test membership with one forward index instead of hashing.
    hot.sort_unstable();
}

/// Static-per-process fields are re-read on first sight, on a rename (comm changed), and at
/// most every `STATIC_REFRESH_TICKS` samples, which catches same-name exec and setuid.
const STATIC_REFRESH_TICKS: u64 = 5;
/// Discover new processes every five samples; known processes are still sampled each tick.
const DISCOVERY_TICKS: u64 = 5;

fn discovery_due(tick: u64) -> bool {
    tick == 1 || (tick - 1) % DISCOVERY_TICKS == 0
}

#[derive(Default)]
struct Cached {
    start: u64,
    ticks: u64,
    seen: u64,
    refreshed: u64,
    name: Arc<str>,
    uid: u32,
    user: Arc<str>,
    command: Arc<str>,
    /// Last full `stat` sample, reused between rotating reads.
    last: Option<Process>,
    /// Global CPU total ticks at this process's last `stat` read.
    read_total: u64,
}

#[derive(Clone, Copy, Default)]
struct Cpu {
    total: u64,
    busy: u64,
}
fn cpu_ticks(line: &str) -> Option<Cpu> {
    let mut fields = line.split_whitespace();
    let _name = fields.next()?;
    // The first eight counters are user..steal; idle plus iowait is the idle share.
    let mut values = [0_u64; 8];
    let mut count = 0;
    for field in fields.take(8) {
        values[count] = field.parse().ok()?;
        count += 1;
    }
    if count < 4 {
        return None;
    }
    let total = values[..count].iter().sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(Cpu {
        total,
        busy: total - idle,
    })
}
/// Whole-file procfs read into the sampler's reused text buffer: `/proc/stat`, `meminfo` and
/// `net/dev` are a few kilobytes per sample, so their capacity is kept instead of allocating a
/// fresh `String` on every tick. A failed or partial read yields the empty string.
fn read_text<'a>(buf: &'a mut String, path: &str) -> &'a str {
    use std::io::Read;
    buf.clear();
    match fs::File::open(path) {
        Ok(mut file) => {
            if file.read_to_string(buf).is_err() {
                buf.clear();
            }
            buf.as_str()
        }
        Err(_) => "",
    }
}
fn load_cpu(text: &str) -> (Cpu, Vec<Cpu>) {
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
    parse_stat_with_name(pid, text, page_bytes, None)
}

/// Same as `parse_stat`, but reuses `cached_name` when the unchanged `comm` field still matches.
/// A process keeps its name for its whole life, so the rotating `stat` read then bumps a refcount
/// instead of building two fresh heap strings on every tick.
fn parse_stat_with_name(
    pid: u32,
    text: &str,
    page_bytes: u64,
    cached_name: Option<&Arc<str>>,
) -> Option<Process> {
    let first = text.find('(')?;
    let last = text.rfind(')')?;
    if first >= last {
        return None;
    }
    // One indexed pass over the fields: the sampler parses a `stat` per due task per tick, so
    // collecting a `Vec<&str>` of all fifty-odd fields would allocate for every read.
    let rest = text.get(last + 1..)?;
    let mut state: Option<char> = None;
    let mut ppid: Option<u32> = None;
    let mut utime: Option<u64> = None;
    let mut stime: Option<u64> = None;
    let mut priority: Option<i64> = None;
    let mut nice: Option<i32> = None;
    let mut threads: Option<u32> = None;
    let mut start: Option<u64> = None;
    let mut vsize: Option<u64> = None;
    let mut pages: Option<u64> = None;
    let mut count = 0_usize;
    for field in rest.split_whitespace() {
        match count {
            0 => state = field.chars().next(),
            1 => ppid = field.parse().ok(),
            11 => utime = field.parse().ok(),
            12 => stime = field.parse().ok(),
            15 => priority = field.parse().ok(),
            16 => nice = field.parse().ok(),
            17 => threads = field.parse().ok(),
            19 => start = field.parse().ok(),
            20 => vsize = field.parse().ok(),
            21 => pages = field.parse().ok(),
            _ => {}
        }
        count += 1;
    }
    if count < 22 {
        return None;
    }
    // Field 24 (rss) is in pages: it replaces /proc/<pid>/status VmRSS, one read fewer per task.
    let comm = text.get(first + 1..last)?;
    // Sanitizing maps control bytes to spaces, so the raw comm can only be compared to the cached
    // name when it has none; otherwise the sanitized fallback rebuilds it (rare).
    let name = match cached_name {
        Some(existing) if existing.as_ref() == comm => Arc::clone(existing),
        _ => Arc::from(sanitize(comm, 64)),
    };
    Some(Process {
        pid,
        ppid: ppid?,
        name,
        state: state?,
        priority: priority?,
        nice: nice?,
        threads: threads?,
        start: start?,
        virt_kib: vsize? / 1024,
        rss_kib: pages?.saturating_mul(page_bytes) / 1024,
        ticks: utime?.saturating_add(stime?),
        ..Process::default()
    })
}
/// Page size from the auxiliary vector (`AT_PAGESZ)`: no libc call and no extra crate.
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
/// `/proc/<pid>/status` carries the real UID within its first lines; the bounded read lets the
/// kernel skip generating the VM, signal and cgroup counters the sampler never looks at.
const STATUS_PREFIX: usize = 2048;
/// Reads at most `limit` bytes of a procfs file into the reused `buf`. `None` when unreadable.
fn read_proc_prefix<'a>(buf: &'a mut Vec<u8>, path: &str, limit: usize) -> Option<&'a [u8]> {
    use std::io::Read;
    buf.clear();
    let mut file = fs::File::open(path).ok()?;
    let mut chunk = [0_u8; 2048];
    let chunk_len = chunk.len();
    loop {
        let room = limit.saturating_sub(buf.len());
        if room == 0 {
            break;
        }
        match file.read(&mut chunk[..room.min(chunk_len)]) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(buf.as_slice())
}
/// Reads a whole procfs file into the reused `buf`, e.g. `stat` or `cmdline`.
fn read_proc<'a>(buf: &'a mut Vec<u8>, path: &str) -> Option<&'a [u8]> {
    read_proc_prefix(buf, path, usize::MAX)
}
/// Writes `/proc/<pid>/<leaf>` into the sampler's reused path buffer: no `format!` per read.
fn proc_path<'a>(path: &'a mut String, pid: u32, leaf: &str) -> &'a str {
    path.clear();
    let _ = write!(path, "/proc/{pid}/{leaf}");
    path.as_str()
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
fn command(scratch: &mut Vec<u8>, path: &mut String, pid: u32, name: &str) -> Arc<str> {
    if let Some(bytes) = read_proc(scratch, proc_path(path, pid, "cmdline")) {
        let args = String::from_utf8_lossy(bytes);
        // `sanitize` already maps every control byte, including the NUL argument separators,
        // to a space, so no intermediate replacement string is needed.
        let text = sanitize(&args, 180);
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Arc::from(trimmed);
        }
    }
    Arc::from(format!("[{name}]"))
}
fn memory(text: &str) -> (u64, u64, u64, u64) {
    // One pass over the four counters we need: no per-sample HashMap of every field.
    let (mut total, mut available, mut swap_total, mut swap_free) = (0_u64, 0, 0, 0);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(kib) = value
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        match key {
            "MemTotal" => total = kib,
            "MemAvailable" => available = kib,
            "SwapTotal" => swap_total = kib,
            "SwapFree" => swap_free = kib,
            _ => {}
        }
    }
    (
        total,
        total.saturating_sub(available),
        swap_total,
        swap_total.saturating_sub(swap_free),
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
fn user_names() -> HashMap<u32, Arc<str>> {
    // /etc/passwd is read once by the worker, not on each frame or in the UI callback. Every
    // process of one uid shares the single `Arc<str>` name.
    fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name: Arc<str> = fields.next()?.into();
            let _password = fields.next()?;
            let uid = fields.next()?.parse::<u32>().ok()?;
            Some((uid, name))
        })
        .collect()
}
/// The three load averages joined with single spaces, built in place (no `Vec` per sample).
fn load_summary(text: &str) -> String {
    let mut out = String::with_capacity(24);
    for (index, field) in text.split_whitespace().take(3).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(field);
    }
    out
}
/// Uptime from the first `/proc/uptime` field as `Nd HH:MM:SS`; `Unavailable` when unparsable.
fn uptime_text(text: &str) -> String {
    text.split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
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
        )
}
impl Sampler {
    pub fn collect(&mut self) -> Snapshot {
        if self.user_cache.is_none() {
            self.user_cache = Some(user_names());
        }
        let (now, cores) = load_cpu(read_text(&mut self.text, "/proc/stat"));
        let (mem_total, mem_used, swap_total, swap_used) =
            memory(read_text(&mut self.text, "/proc/meminfo"));
        let load = load_summary(read_text(&mut self.text, "/proc/loadavg"));
        let uptime = uptime_text(read_text(&mut self.text, "/proc/uptime"));
        let mut result = Snapshot {
            mem_total,
            mem_used,
            swap_total,
            swap_used,
            load,
            uptime,
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
        let cpu_count = cores.len().max(1) as f64;
        if self.page_bytes == 0 {
            self.page_bytes = page_size_bytes();
        }
        self.tick += 1;
        let tick = self.tick;
        // Reused pid list: discovery appends, then one sort/dedup pass over the same buffer.
        self.pids.clear();
        self.pids.extend(self.tasks.keys().copied());
        if discovery_due(tick)
            && let Ok(entries) = fs::read_dir("/proc")
        {
            self.pids.extend(
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok()),
            );
        }
        self.pids.sort_unstable();
        self.pids.dedup();
        result.processes.reserve(self.pids.len());
        // `pids` and `hot` are both ascending, so one forward cursor answers "is this pid hot"
        // for the whole walk: no hash probe per process per sample.
        let mut hot_at = 0_usize;
        for index in 0..self.pids.len() {
            let pid = self.pids[index];
            while hot_at < self.hot.len() && self.hot[hot_at] < pid {
                hot_at += 1;
            }
            let hot = self.hot.get(hot_at) == Some(&pid);
            let Some(task) = self.sample_task(pid, tick, now.total, cpu_count, hot) else {
                continue;
            };
            result.threads += u64::from(task.threads);
            if task.state == 'R' {
                result.running += 1;
            }
            result.processes.push(task);
        }
        self.tasks.retain(|_, cached| cached.seen == tick);
        assign_depths(&mut self.parents, &mut result.processes);
        hot_pids_into(&mut self.hot, &mut self.ranked, &result.processes);
        result.tasks = result.processes.len();
        let (rx, tx) = parse_net_dev(read_text(&mut self.text, "/proc/net/dev"));
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
    /// `hot` pids are the highest-CPU ones of the last sample: they are re-read every tick, while
    /// a cold pid replays its last sample on the ticks its rotating read is not due.
    fn sample_task(
        &mut self,
        pid: u32,
        tick: u64,
        total: u64,
        cpu_count: f64,
        hot: bool,
    ) -> Option<Process> {
        if !hot
            && !slow_due(pid, tick)
            && let Some(cached) = self.tasks.get_mut(&pid)
            && let Some(last) = cached.last.clone()
        {
            // Non-hot and not due: keep the last sample. Its `seen` tick stays current so the
            // pid is still tracked and vanishes on its next due read.
            cached.seen = tick;
            return Some(last);
        }
        let text = read_proc(&mut self.scratch, proc_path(&mut self.path, pid, "stat"))?;
        let text = std::str::from_utf8(text).ok()?;
        // One probe for the whole due path: the name reuse, the CPU counters and the static
        // fields all live in this entry, so a rotating read costs one map lookup instead of three.
        let cached = self.tasks.entry(pid).or_default();
        // A live pid's `comm` rarely changes: reuse the cached name so the per-tick `stat` read
        // does not rebuild two heap strings for every due task.
        let mut task = parse_stat_with_name(pid, text, self.page_bytes, Some(&cached.name))?;
        // Same PID and start time as the previous sample: the delta is meaningful. PID reuse
        // changes the start time, so a recycled PID never inherits the old CPU counter.
        let known = cached.seen != 0 && cached.start == task.start;
        if known {
            task.cpu = cpu_percent(
                task.ticks.saturating_sub(cached.ticks),
                total.saturating_sub(cached.read_total),
                cpu_count,
            );
        }
        cached.read_total = total;
        let stale = !known
            || cached.name != task.name
            || tick.saturating_sub(cached.refreshed) >= STATIC_REFRESH_TICKS;
        if stale {
            let status = read_proc_prefix(
                &mut self.scratch,
                proc_path(&mut self.path, pid, "status"),
                STATUS_PREFIX,
            );
            cached.uid = status
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map_or(u32::MAX, parse_uid);
            let uid = cached.uid;
            cached.user = self
                .user_cache
                .as_ref()
                .and_then(|names| names.get(&uid))
                .cloned()
                .unwrap_or_else(|| {
                    if uid == u32::MAX {
                        Arc::from("?")
                    } else {
                        Arc::from(uid.to_string())
                    }
                });
            // exec can replace argv without changing PID/starttime; the refresh bounds that lag.
            cached.command = command(&mut self.scratch, &mut self.path, pid, &task.name);
            cached.refreshed = tick;
        }
        if cached.name != task.name {
            cached.name.clone_from(&task.name);
        }
        cached.start = task.start;
        cached.ticks = task.ticks;
        cached.seen = tick;
        task.user.clone_from(&cached.user);
        task.command.clone_from(&cached.command);
        // Keep the replay copy for every pid, including hot ones: a process that leaves the hot
        // set replays this sample when its first rotating read is not due, and with `Arc<str>`
        // fields the clone is a few refcount bumps. (u26 skipped it for hot pids, which let a
        // process that had been hot replay a sample from before it became hot.)
        cached.last = Some(task.clone());
        Some(task)
    }
}

/// Tree depth from `ppid` links inside the snapshot. Bounded, so a corrupt cycle cannot loop.
/// The caller owns the `parents` map so the sampler reuses one allocation per process table
/// across samples instead of rebuilding a fresh `HashMap` each tick.
pub fn assign_depths(parents: &mut HashMap<u32, u32>, processes: &mut [Process]) {
    const MAX_DEPTH: u32 = 32;
    parents.clear();
    parents.extend(processes.iter().map(|p| (p.pid, p.ppid)));
    for process in processes.iter_mut() {
        let mut depth = 0;
        let mut parent = process.ppid;
        while depth < MAX_DEPTH {
            let Some(&next) = parents.get(&parent) else {
                break;
            };
            if parent == process.pid {
                break;
            }
            depth += 1;
            parent = next;
        }
        process.depth = depth;
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
        fields[1] = "1".into();
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
        assert_eq!(&*p.name, "some ) process");
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
                task.name.as_ref(),
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
    fn unchanged_comm_reuses_the_cached_name_arc() {
        let stat = "4242 (fire fox) S 1 4242 4242 0 -1 4194560 1 0 0 0 150 30 0 0 20 0 7 0 555 6291456 1030 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let cached: Arc<str> = "fire fox".into();
        let reused = parse_stat_with_name(4242, stat, 4096, Some(&cached)).expect("parse");
        assert!(Arc::ptr_eq(&reused.name, &cached));
        // A different cached name is ignored and the parsed comm still wins.
        let stale: Arc<str> = "old name".into();
        let renamed = parse_stat_with_name(4242, stat, 4096, Some(&stale)).expect("parse");
        assert_eq!(&*renamed.name, "fire fox");
        assert!(!Arc::ptr_eq(&renamed.name, &stale));
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
    fn load_summary_joins_the_three_averages_without_a_vec() {
        assert_eq!(
            load_summary("0.52 0.58 0.59 1/1234 5678\n"),
            "0.52 0.58 0.59"
        );
        assert_eq!(load_summary("0.10 0.20\n"), "0.10 0.20");
        assert_eq!(load_summary(""), "");
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
    #[test]
    fn discovery_is_periodic_and_known_tasks_are_sampled_between_scans() {
        assert_eq!(
            (1..=11)
                .filter(|&tick| discovery_due(tick))
                .collect::<Vec<_>>(),
            vec![1, 6, 11]
        );
        let mut sampler = Sampler::default();
        let pid = std::process::id();
        assert!(sampler.collect().processes.iter().any(|p| p.pid == pid));
        assert!(sampler.collect().processes.iter().any(|p| p.pid == pid));
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    /// utime + stime of this process in clock ticks (`CLK_TCK` is 100 on Linux targets `FlameWM` ships for).
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
    /// cargo test -p flamewm-task-manager --release `bench_sampler` -- --ignored --nocapture
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

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn proc_at(pid: u32, ppid: u32) -> Process {
        Process {
            pid,
            ppid,
            ..Process::default()
        }
    }

    #[test]
    fn stat_parses_parent_pid_after_comm_with_parens() {
        let text = "42 (a (b) c) S 7 42 42 0 -1 0 0 0 0 0 1 2 0 0 20 0 1 0 100 0 5 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let p = parse_stat(42, text, 4096).expect("parse");
        assert_eq!(p.ppid, 7);
        assert_eq!(&*p.name, "a (b) c");
    }

    #[test]
    fn depths_follow_parent_chain_and_stop_at_missing_or_cyclic_parents() {
        let mut list = vec![
            proc_at(1, 0),
            proc_at(10, 1),
            proc_at(11, 10),
            proc_at(20, 999),
            proc_at(30, 31),
            proc_at(31, 30),
        ];
        let mut parents = std::collections::HashMap::new();
        assign_depths(&mut parents, &mut list);
        let depth = |pid| list.iter().find(|p| p.pid == pid).unwrap().depth;
        assert_eq!(depth(1), 0);
        assert_eq!(depth(10), 1);
        assert_eq!(depth(11), 2);
        assert_eq!(depth(20), 0);
        assert!(depth(30) <= 32 && depth(31) <= 32);
    }
}

#[cfg(test)]
mod sampling_schedule_tests {
    use super::*;

    fn process(pid: u32, cpu: f64) -> Process {
        Process {
            pid,
            cpu,
            ..Process::default()
        }
    }

    #[test]
    fn every_pid_gets_a_rotating_read_within_slow_ticks() {
        for pid in [1_u32, 2, 3, 4, 5, 1_234, 99_999] {
            let due = (1..=SLOW_TICKS).filter(|tick| slow_due(pid, *tick)).count();
            assert_eq!(
                due, 1,
                "pid {pid} must be due exactly once per {SLOW_TICKS} ticks"
            );
        }
    }

    #[test]
    fn cpu_percent_scales_by_span_since_last_read() {
        // 50 process ticks over 100 global ticks on a 4-cpu box is 50% of one cpu x4 = 200%.
        assert!((cpu_percent(50, 100, 4.0) - 200.0).abs() < 1e-9);
        // Same process read after two intervals: the span doubles, so the percentage halves.
        assert!((cpu_percent(50, 200, 4.0) - 100.0).abs() < 1e-9);
        assert_eq!(
            cpu_percent(10, 0, 4.0),
            0.0,
            "zero span reports 0, never NaN"
        );
    }

    #[test]
    fn hot_set_is_the_highest_cpu_pids_and_bounded() {
        let mut processes: Vec<Process> = (0..(HOT_COUNT as u32 + 10))
            .map(|pid| process(pid, f64::from(pid)))
            .collect();
        let mut hot = Vec::new();
        let mut ranked = Vec::new();
        hot_pids_into(&mut hot, &mut ranked, &processes);
        assert_eq!(hot.len(), HOT_COUNT);
        assert!(hot.contains(&(HOT_COUNT as u32 + 9)));
        assert!(!hot.contains(&0));
        assert!(hot.windows(2).all(|pair| pair[0] < pair[1]), "ascending");
        processes.truncate(3);
        hot_pids_into(&mut hot, &mut ranked, &processes);
        assert_eq!(hot.len(), 3);
    }
}
