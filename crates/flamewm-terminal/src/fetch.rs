//! One-shot, native system summary. No polling, subprocesses, or shell interpolation.
//! The same code serves the `flamefetch` command and FlameWM Terminal's initial shell.
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const BUILTIN_LOGO: &str = include_str!("../../../assets/flamefetch/logo.txt");
const COMPACT_LOGO: &str = "        .::::.\n     .:++++++++:.\n   .:++++++++++++:.\n  :+++++::...:++++:\n :++++:     :++++:\n :+++:    .:++++:\n :++:    .:++++:\n  :+.   .:++++:\n       .:++++:\n     .:++++:\n    :++++:\n   :+++:\n  :++:\n  :::";
const DEFAULT_MODULES: &[&str] = &[
    "os", "host", "kernel", "uptime", "packages", "shell", "display",
    "de", "wm", "terminal", "cpu", "gpu", "memory", "swap", "disk",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout { Auto, Compact, Full }

#[derive(Clone, Debug)]
struct Config {
    startup: bool,
    logo: String,
    logo_color: String,
    key_color: String,
    layout: Layout,
    palette: bool,
    modules: Vec<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self { startup: true, logo: "builtin".into(), logo_color: "#EF4048".into(),
            key_color: "#EF4048".into(), layout: Layout::Auto, palette: true,
            modules: DEFAULT_MODULES.iter().map(|s| (*s).into()).collect() }
    }
}
fn config_path() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(base).join("flamewm/flamefetch.toml")
    } else { home().join(".config/flamewm/flamefetch.toml") }
}
fn home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")) }
fn dequote(text: &str) -> &str {
    text.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(text.trim())
}
fn parse_modules(text: &str) -> Vec<String> {
    text.trim().trim_start_matches('[').trim_end_matches(']').split(',')
        .map(dequote).filter(|s| !s.is_empty() && s.len() <= 30)
        .take(32).map(str::to_owned).collect()
}
impl Config {
    fn load(path: &Path) -> Self {
        let mut config = Self::default();
        let Ok(raw) = fs::read_to_string(path) else { return config };
        if raw.len() > 32 * 1024 { return config; }
        let mut collecting_modules = String::new();
        let mut in_modules = false;
        for line in raw.lines() {
            // A hash inside a quoted RGB value is data, not a TOML comment.
            let mut quoted = false;
            let cutoff = line.char_indices().find_map(|(i, c)| {
                if c == '"' { quoted = !quoted; }
                (c == '#' && !quoted).then_some(i)
            }).unwrap_or(line.len());
            let line = line[..cutoff].trim();
            if in_modules {
                collecting_modules.push(' ');
                collecting_modules.push_str(line);
                if line.contains(']') {
                    config.modules = parse_modules(&collecting_modules);
                    collecting_modules.clear();
                    in_modules = false;
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "startup" => if let Ok(v) = value.parse::<bool>() { config.startup = v },
                "palette" => if let Ok(v) = value.parse::<bool>() { config.palette = v },
                "logo" => config.logo = dequote(value).chars().take(512).collect(),
                "logo_color" => config.logo_color = dequote(value).to_owned(),
                "key_color" => config.key_color = dequote(value).to_owned(),
                "layout" => config.layout = match dequote(value) {
                    "full" => Layout::Full, "compact" => Layout::Compact, _ => Layout::Auto,
                },
                "modules" => {
                    if value.contains(']') { config.modules = parse_modules(value) }
                    else { in_modules = true; collecting_modules.push_str(value); }
                }
                _ => (),
            }
        }
        config
    }
}

/// Whether a newly opened interactive terminal should show its one-time summary.
#[must_use]
pub fn startup_enabled() -> bool { Config::load(&config_path()).startup }

/// Run the fetch in the child's controlling PTY, then replace this process with the actual user shell.
/// No interactive shell config is modified; command-line `-e` sessions bypass this path entirely.
pub fn exec_shell(shell: &str, args: &[String]) -> ! {
    let _ = render(&mut io::stdout().lock(), &Config::load(&config_path()), true, true);
    let error = Command::new(shell).args(args).exec();
    eprintln!("flamewm-terminal: cannot execute shell {shell}: {error}");
    std::process::exit(127)
}

/// Invoked by a standalone `flamefetch` binary or the installed FlameWM multi-call link.
pub fn cli_main(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut config_file = config_path();
    let mut arguments = args.into_iter().peekable();
    let mut overrides: Vec<(String, String)> = Vec::new();
    while let Some(arg) = arguments.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("flamefetch — native one-shot Linux system information\n  --no-logo     Hide logo\n  --logo PATH   Custom ASCII logo\n  --layout auto|compact|full\n  --config PATH Configuration file\n  --no-color    Disable ANSI colors\n  --no-palette  Hide terminal color blocks\n  --version     Print version");
                return Ok(());
            }
            "--version" | "-V" => { println!("flamefetch {}", env!("CARGO_PKG_VERSION")); return Ok(()) }
            "--no-logo" => overrides.push(("logo".into(), "none".into())),
            "--no-color" => overrides.push(("color".into(), "none".into())),
            "--no-palette" => overrides.push(("palette".into(), "false".into())),
            "--logo" | "--layout" | "--config" => {
                let value = arguments.next().ok_or_else(|| format!("{arg} requires a value"))?;
                if arg == "--config" { config_file = PathBuf::from(value) }
                else { overrides.push((arg.trim_start_matches("--").into(), value)) }
            }
            _ => return Err(format!("unknown option {arg}; use --help")),
        }
    }
    let mut config = Config::load(&config_file);
    let mut colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    for (key, value) in overrides {
        match key.as_str() {
            "logo" => config.logo = value,
            "palette" => config.palette = value == "true",
            "color" => colored = value != "none",
            "layout" => config.layout = match value.as_str() { "full" => Layout::Full, "compact" => Layout::Compact, "auto" => Layout::Auto, _ => return Err("invalid layout".into()) },
            _ => (),
        }
    }
    render(&mut io::stdout().lock(), &config, colored, io::stdout().is_terminal())
        .map_err(|e| e.to_string())
}

fn read_small(path: &str, max: usize) -> String {
    let Ok(mut file) = File::open(path) else { return String::new() };
    let mut data = Vec::new();
    let _ = file.take(max as u64).read_to_end(&mut data);
    String::from_utf8_lossy(&data).into_owned()
}
fn field(path: &str, key: &str) -> Option<String> {
    read_small(path, 16 * 1024).lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name == key).then(|| dequote(value).to_owned())
    })
}
fn mem_values() -> (u64,u64,u64,u64) {
    let mut vals=[0_u64;4];
    for line in read_small("/proc/meminfo", 8192).lines() {
        if let Some((key, rest)) = line.split_once(':') {
            if let Some(i) = ["MemTotal", "MemAvailable", "SwapTotal", "SwapFree"].iter().position(|s| *s==key) {
                vals[i]=rest.split_whitespace().next().and_then(|s|s.parse().ok()).unwrap_or(0);
            }
        }
    }
    (vals[0],vals[0].saturating_sub(vals[1]),vals[2],vals[2].saturating_sub(vals[3]))
}
fn cpu_name() -> String {
    for line in read_small("/proc/cpuinfo", 128*1024).lines() {
        if let Some(v) = line.strip_prefix("model name") {
            return v.split_once(':').map_or("", |(_,s)|s.trim()).to_owned();
        }
    }
    String::new()
}
fn gpu_name() -> String {
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten().take(32) {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("card") || name.to_string_lossy().contains('-') { continue; }
            let vendor = fs::read_to_string(entry.path().join("device/vendor")).unwrap_or_default();
            let device = fs::read_to_string(entry.path().join("device/device")).unwrap_or_default();
            let maker = match vendor.trim() {
                "0x8086" => "Intel", "0x10de" => "NVIDIA", "0x1002" => "AMD", "0x1af4" => "Virtio", _ => "GPU",
            };
            if !vendor.is_empty() { return format!("{maker} {}",device.trim()) }
        }
    }
    String::new()
}
fn package_count() -> String {
    let Ok(file) = File::open("/var/lib/dpkg/status") else { return "N/A".into() };
    let reader=BufReader::new(file);
    let mut count = 0_u32;
    for line in reader.lines().take(750_000).flatten() {
        if line == "Status: install ok installed" { count+=1; }
    }
    format!("{count} (dpkg)")
}
fn disk() -> String {
    // statvfs is already provided by rustix in the FlameWM terminal's dependency set.
    if let Ok(stats)=rustix::fs::statvfs("/") {
        let total = stats.f_blocks.saturating_mul(stats.f_frsize);
        let free = stats.f_bavail.saturating_mul(stats.f_frsize);
        return format!("{:.1} / {:.1} GiB", gb(total.saturating_sub(free)),gb(total));
    }
    "N/A".into()
}
fn gb(bytes: u64) -> f64 { bytes as f64 / 1_073_741_824.0 }
fn uptime() -> String {
    let s = read_small("/proc/uptime", 80);
    let seconds = s.split_whitespace().next().and_then(|s|s.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    format!("{}d {}h {}m",seconds/86400,(seconds/3600)%24,(seconds/60)%60)
}
fn kern() -> String { read_small("/proc/sys/kernel/osrelease", 256).trim().to_owned() }
fn host() -> String { read_small("/sys/class/dmi/id/product_name", 256).trim().to_owned() }
fn shell_name() -> String { std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()).rsplit('/').next().unwrap_or("sh").to_owned() }
fn get_module(name: &str, memory: (u64, u64, u64, u64)) -> Option<(String, String)> {
    let (total, used, swap, swap_used) = memory;
    let result = match name {
        "os" => field("/etc/os-release", "PRETTY_NAME").unwrap_or_else(|| "Linux".into()),
        "host" => host(), "kernel" => kern(), "uptime" => uptime(),
        "packages" => package_count(), "shell" => shell_name(),
        "display" => std::env::var("DISPLAY").unwrap_or_default(),
        "de" | "wm" => "FlameWM".into(), "terminal" => "FlameWM Terminal".into(),
        "cpu" => cpu_name(), "gpu" => gpu_name(),
        "memory" => format!("{:.2} / {:.2} GiB",gb(used*1024),gb(total*1024)),
        "swap" => format!("{:.2} / {:.2} GiB",gb(swap_used*1024),gb(swap*1024)),
        "disk" => disk(),
        _ => return None,
    };
    (!result.is_empty()).then(|| (name.to_uppercase(),result))
}
fn truecolor(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len()!=6 || !hex.bytes().all(|b|b.is_ascii_hexdigit()) { return None }
    let n=u32::from_str_radix(hex,16).ok()?;
    Some(format!("\x1b[38;2;{};{};{}m", (n>>16)&255, (n>>8)&255, n&255))
}
fn columns() -> usize {
    std::env::var("COLUMNS").ok().and_then(|v|v.parse::<usize>().ok())
        .or_else(|| std::env::var("FLAMEFETCH_COLUMNS").ok().and_then(|v|v.parse().ok()))
        .unwrap_or(100).clamp(20,400)
}
fn logo_lines(config: &Config, cols: usize) -> Vec<String> {
    if config.logo == "none" { return Vec::new() }
    let raw = if config.logo=="builtin" { BUILTIN_LOGO.to_owned() }
        else { let path = Path::new(&config.logo);
            match fs::metadata(path) { Ok(meta) if meta.is_file() && meta.len()<=32*1024 => read_small(config.logo.as_str(),32*1024), _=>String::new() }
        };
    let full: Vec<String> = raw.lines().map(str::trim_end).filter(|s|!s.is_empty()).take(64).map(str::to_owned).collect();
    let width=full.iter().map(|s|s.chars().count()).max().unwrap_or(0);
    let use_full = match config.layout { Layout::Full=>true, Layout::Compact=>false, Layout::Auto=>width+44 <= cols && full.len()<=28 };
    if use_full { full } else if config.logo=="builtin" { COMPACT_LOGO.lines().map(str::to_owned).collect() } else { full }
}
fn render(writer: &mut impl Write, config: &Config, colored: bool, tty: bool) -> io::Result<()> {
    let memory = mem_values();
    let pairs:Vec<_>=config.modules.iter().filter_map(|s|get_module(s, memory)).collect();
    let width=columns();
    let logo=logo_lines(config,width);
    let max_logo_width=logo.iter().map(|s|s.chars().count()).max().unwrap_or(0);
    let side_by_side=!logo.is_empty() && width>=max_logo_width+32;
    let (logo_ansi,key_ansi)=if colored {
        (truecolor(&config.logo_color).unwrap_or_default(), truecolor(&config.key_color).unwrap_or_default())
    } else { (String::new(),String::new()) };
    let reset=if colored { "\x1b[0m" } else { "" };
    let header=format!("{}@{}", std::env::var("USER").unwrap_or_default(), read_small("/proc/sys/kernel/hostname",256).trim());
    let info:Vec<String>=std::iter::once(format!("{key_ansi}{header}{reset}"))
        .chain(std::iter::once("--------------------------".into()))
        .chain(pairs.into_iter().map(|(key,value)|format!("{key_ansi}{key}:{reset} {value}")))
        .collect();
    if side_by_side {
        for row in 0..logo.len().max(info.len()) {
            let left=logo.get(row).map(String::as_str).unwrap_or("");
            write!(writer,"{logo_ansi}{left}{reset}{:width$}","",width=max_logo_width.saturating_sub(left.chars().count())+3)?;
            if let Some(right)=info.get(row) { write!(writer,"{right}")?; }
            writeln!(writer)?;
        }
    } else {
        for line in logo { writeln!(writer,"{logo_ansi}{line}{reset}")?; }
        for line in info { writeln!(writer,"{line}")?; }
    }
    if colored && config.palette && tty {
        let blocks=[40,41,42,43,44,45,46,47];
        for bright in [false,true] {
            for code in blocks { write!(writer,"\x1b[{}m  ",code+if bright {60} else {0})?; }
            writeln!(writer,"\x1b[0m")?;
        }
    }
    writeln!(writer)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_logo_fits_terminal() {
        let c=Config::default();
        assert!(logo_lines(&c,80).iter().map(|s|s.chars().count()).max().unwrap() < 32);
    }
    #[test]
    fn no_logo_preserves_data() {
        let mut c=Config::default();c.logo="none".into();c.palette=false;
        let mut output=Vec::new();render(&mut output,&c,false,false).unwrap();
        let out=String::from_utf8(output).unwrap();
        assert!(out.contains("OS:") && !out.contains("\x1b["));
    }
    #[test]
    fn colors_accept_only_rgb_hex() {
        assert!(truecolor("#ef4048").is_some());
        assert!(truecolor("blue").is_none());
    }
}
