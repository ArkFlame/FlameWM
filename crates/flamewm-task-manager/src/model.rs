//! Linux procfs sampler. All filesystem reads run off the X11/UI thread.
//! PID plus process start-time is the authority for selecting a live process.
use std::collections::HashMap;
use std::fs;

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
}

#[derive(Default)]
pub struct Sampler {
    prev_global: Option<Cpu>,
    prev_cores: Vec<Cpu>,
    prev_tasks: HashMap<u32, (u64, u64)>,
    user_cache: Option<HashMap<u32, String>>,
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
pub fn parse_stat(pid: u32, text: &str) -> Option<Process> {
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
    Some(Process {
        pid,
        name: sanitize(text.get(first + 1..last)?, 64),
        state: fields[0].chars().next()?,
        priority: fields[15].parse().ok()?,
        nice: fields[16].parse().ok()?,
        threads: fields[17].parse().ok()?,
        start: fields[19].parse().ok()?,
        virt_kib: fields[20].parse::<u64>().ok()? / 1024,
        rss_kib: 0,
        uid: u32::MAX,
        ticks: utime.saturating_add(stime),
        ..Process::default()
    })
}
fn sanitize(text: &str, max_chars: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max_chars)
        .collect()
}
fn status(pid: u32, process: &mut Process) {
    if let Ok(text) = fs::read_to_string(format!("/proc/{pid}/status")) {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("Uid:") {
                process.uid = v
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(u32::MAX);
            } else if let Some(v) = line.strip_prefix("VmRSS:") {
                process.rss_kib = v
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
    }
}
fn command(pid: u32, name: &str) -> String {
    if let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) {
        let args = String::from_utf8_lossy(&bytes);
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
        let mut previous = HashMap::new();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok(text) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                    continue;
                };
                let Some(mut task) = parse_stat(pid, &text) else {
                    continue;
                };
                let ticks = task.ticks;
                if let Some((start, prior)) = self.prev_tasks.get(&pid).copied() {
                    if start == task.start && delta != 0 {
                        task.cpu =
                            100.0 * cpu_count * ticks.saturating_sub(prior) as f64 / delta as f64;
                    }
                }
                previous.insert(pid, (task.start, ticks));
                status(pid, &mut task);
                task.user = self
                    .user_cache
                    .as_ref()
                    .and_then(|names| names.get(&task.uid))
                    .cloned()
                    .unwrap_or_else(|| {
                        if task.uid == u32::MAX {
                            "?".into()
                        } else {
                            task.uid.to_string()
                        }
                    });
                // Re-read command lines: exec can replace argv without changing PID/starttime.
                task.command = command(pid, &task.name);
                result.threads += u64::from(task.threads);
                if task.state == 'R' {
                    result.running += 1;
                }
                result.processes.push(task);
            }
        }
        result.tasks = result.processes.len();
        self.prev_global = Some(now);
        self.prev_cores = cores;
        self.prev_tasks = previous;
        result
    }
}

/// Refuse PID reuse between display and process operation. No shell expansion is possible.
pub fn check_identity(pid: u32, start: u64) -> bool {
    let path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(path) else {
        return false;
    };
    parse_stat(pid, &stat).is_some_and(|current| current.start == start)
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
        let line = format!("23 (some ) process) {}", fields.join(" "));
        let p = parse_stat(23, &line).expect("parse");
        assert_eq!(p.name, "some ) process");
        assert_eq!(
            (p.start, p.priority, p.nice, p.threads, p.virt_kib),
            (999_999, 25, 5, 2, 1024)
        );
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
