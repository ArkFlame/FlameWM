//! PTY ownership: master/slave pair (rustix, safe API), child spawn on the slave, a writer thread so key input never
//! blocks the UI, and TIOCSWINSZ on resize. The child acquires the slave as its controlling terminal through
//! util-linux `setsid -c`, so Ctrl+C and job control work without raw FFI in this crate.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

/// Grace period for a hung-up child to flush its own state before the hard kill. An interactive
/// shell writes `$HISTFILE` from its SIGHUP handler; the wait is the same one Terminator uses
/// (`.vendor/terminator-master/terminatorlib/terminal.py`, `_wait_for_shell_exit`).
const HANGUP_GRACE: Duration = Duration::from_millis(250);
/// Sampling interval for the pre-pidfd fallback only. Kernels with `pidfd_open` (Linux 5.3+)
/// block in `poll` on the child's pidfd instead, so a close costs one wait, not a wake every
/// few milliseconds.
const HANGUP_POLL: Duration = Duration::from_millis(5);

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

/// Terminal size in cells plus the pixel size reported to the child (TIOCSWINSZ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

pub struct PtyProcess {
    child: Child,
    master: File,
    writer: Sender<Vec<u8>>,
    reader: Option<File>,
}

/// What to run inside the terminal. `-e CMD...` supplies `program`/`args`; otherwise `$SHELL` runs with no args.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
}

impl Launch {
    /// The user's shell (`$SHELL`, else `/bin/sh`), started in `cwd`.
    #[must_use]
    pub fn shell(cwd: Option<std::path::PathBuf>) -> Self {
        let program = std::env::var("SHELL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        // Interactive line editing, history, and completion belong to the selected shell.
        // Explicit `-i` also covers users who launch the terminal from a noninteractive session.
        let args = vec!["-i".into()];
        Self { program, args, cwd }
    }
}

impl PtyProcess {
    /// Opens a PTY pair, sizes it, and starts `launch` on the slave with a controlling terminal when `setsid` exists.
    pub fn spawn(launch: &Launch, size: PtySize) -> io::Result<Self> {
        // Close-on-exec: a child holding the master never sees the terminal hang up, so closing the window would
        // leave the shell and everything it started running.
        let master: OwnedFd =
            openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
        grantpt(&master)?;
        unlockpt(&master)?;
        let slave_name = ptsname(&master, Vec::new())?;
        let slave_path = Path::new(std::ffi::OsStr::new(
            slave_name
                .to_str()
                .map_err(|error| io::Error::other(error.to_string()))?,
        ));
        tcsetwinsize(&master, winsize(size))?;
        let master = File::from(master);
        // NOCTTY: a terminal started as a session leader (setsid launchers) would otherwise take the slave as its
        // own controlling terminal and get SIGHUP when the shell's session claims it.
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NOCTTY.bits() as i32)
            .open(slave_path)?;
        let child = spawn_child(launch, &slave)?;
        drop(slave);
        let reader = master.try_clone()?;
        let mut writable = master.try_clone()?;
        let (writer, queue) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("flamewm-terminal-write".into())
            .spawn(move || {
                for bytes in queue {
                    if writable
                        .write_all(&bytes)
                        .and_then(|()| writable.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            master,
            writer,
            reader: Some(reader),
        })
    }

    /// Line-discipline flags the prompt check needs, from one ioctl: `(canonical, echoing)`. Shell line editors clear
    /// ICANON while they read keys at the prompt, so a set flag means a program reads cooked lines (sudo, cat, a REPL);
    /// a password prompt clears echo too, so both flags are needed. Errors count as the cooked default.
    ///
    /// Costs a syscall, so the session caches the pair and refreshes it per output batch and per key press, never per
    /// frame.
    #[must_use]
    pub fn line_modes(&self) -> (bool, bool) {
        rustix::termios::tcgetattr(&self.master).map_or((true, true), |attrs| {
            (
                attrs
                    .local_modes
                    .contains(rustix::termios::LocalModes::ICANON),
                attrs
                    .local_modes
                    .contains(rustix::termios::LocalModes::ECHO),
            )
        })
    }

    /// The master read side, owned by the reader thread. Returns `None` after the first call.
    pub fn take_reader(&mut self) -> Option<File> {
        self.reader.take()
    }

    /// Queues bytes for the child; the writer thread performs the blocking write.
    pub fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.writer
            .send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "terminal writer stopped"))
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        tcsetwinsize(&self.master, winsize(size)).map_err(io::Error::from)
    }

    /// The kernel's window size for this terminal, which is what the child sees.
    pub fn winsize(&self) -> io::Result<PtySize> {
        let size = rustix::termios::tcgetwinsize(&self.master)?;
        Ok(PtySize {
            cols: size.ws_col,
            rows: size.ws_row,
            pixel_width: size.ws_xpixel,
            pixel_height: size.ws_ypixel,
        })
    }

    /// Ends the child: SIGHUP first so a shell can flush its state, SIGKILL only when it stays.
    ///
    /// Closing the master alone also hangs the session up, but the kernel delivers that SIGHUP
    /// as a burst and bash 5.2+ aborts a history flush that a second terminating signal
    /// interrupts, which loses `$HISTFILE` on every window close. Sending the hangup explicitly
    /// and waiting for the child to exit before the master closes keeps one signal the shell can
    /// finish on (Terminator's `_wait_for_shell_exit` documents the same mechanism). The child is
    /// always reaped, so no zombie survives inside a still-running terminal.
    pub fn kill(&mut self) {
        let _ = hangup_then_reap(&mut self.child);
    }

    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        // Always go through the hangup path: it reaps a child that already exited as well, so no
        // zombie is left behind when a tab closes while the terminal keeps running.
        let _ = hangup_then_reap(&mut self.child);
    }
}

/// Hangs `child` up, waits up to `HANGUP_GRACE` for it to exit, then kills and reaps it. Sends no
/// signal to a child that already exited; the final `wait` reaps it either way.
fn hangup_then_reap(child: &mut Child) -> io::Result<std::process::ExitStatus> {
    if let Ok(None) = child.try_wait() {
        let pid = rustix::process::Pid::from_child(&*child);
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::HUP);
        // Block on the child's pidfd with the grace as the poll timeout: one syscall wait
        // instead of a wake every `HANGUP_POLL`, and none at all once it exits. Kernels before
        // `pidfd_open` (Linux 5.3) fall back to sampling.
        let exited = wait_for_exit(pid, HANGUP_GRACE)
            .unwrap_or_else(|| sample_until_exit(child, HANGUP_GRACE));
        // A child that ignored the hangup (a foreground program with its own handler, or a
        // stopped job) must still be gone before the master closes.
        if !exited {
            let _ = child.kill();
        }
    }
    child.wait()
}

/// Waits up to `grace` for `pid` to exit, blocking in `poll` on its pidfd. `None` when pidfds are
/// unavailable, so the caller can fall back to sampling.
fn wait_for_exit(pid: rustix::process::Pid, grace: Duration) -> Option<bool> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let pidfd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok()?;
    let timeout = Timespec {
        tv_sec: grace.as_secs() as _,
        tv_nsec: grace.subsec_nanos().into(),
    };
    let mut fds = [PollFd::new(&pidfd, PollFlags::IN)];
    loop {
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return Some(false),
            Ok(_) => return Some(true),
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return None,
        }
    }
}

/// Pre-pidfd fallback: samples the child until it exits or `grace` elapses.
fn sample_until_exit(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(HANGUP_POLL),
            // An unreapable child is best left to the final `wait`.
            Err(_) => return true,
        }
    }
    false
}

fn winsize(size: PtySize) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: size.pixel_width,
        ws_ypixel: size.pixel_height,
    }
}

fn spawn_child(launch: &Launch, slave: &File) -> io::Result<Child> {
    let env = |command: &mut Command| {
        command
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "FlameWM");
        if let Some(cwd) = &launch.cwd {
            command.current_dir(cwd);
        }
    };
    let stdio = |command: &mut Command| -> io::Result<()> {
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        Ok(())
    };
    let reset = default_signals_prefix();
    // `setsid -c` makes the slave the controlling terminal; without util-linux the shell still runs, without job control.
    let mut wrapped = Command::new("setsid");
    wrapped
        .arg("-c")
        .args(reset)
        .arg(&launch.program)
        .args(&launch.args);
    env(&mut wrapped);
    stdio(&mut wrapped)?;
    match wrapped.spawn() {
        Ok(child) => Ok(child),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut direct = match reset.split_first() {
                Some((program, args)) => {
                    let mut command = Command::new(program);
                    command.args(args).arg(&launch.program);
                    command
                }
                None => Command::new(&launch.program),
            };
            direct.args(&launch.args);
            env(&mut direct);
            stdio(&mut direct)?;
            direct.spawn()
        }
        Err(error) => Err(error),
    }
}

/// `env --default-signal` (coreutils 8.31+) when available. Ignored signal dispositions survive exec, so a terminal
/// started by a launcher that ignores SIGINT and SIGQUIT (any background job of a non-interactive shell does) would
/// hand that to the shell, where no trap can undo it: Ctrl+C and Ctrl+\ would reach the line discipline and do
/// nothing. Every signal starts at its default in the child instead, as in other terminal emulators.
fn default_signals_prefix() -> &'static [&'static str] {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let supported = *SUPPORTED.get_or_init(|| {
        Command::new("env")
            .args(["--default-signal", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    });
    if supported {
        &["env", "--default-signal"]
    } else {
        &[]
    }
}

/// Reads one chunk; `Ok(0)` or EIO means the child closed the terminal.
pub fn read_chunk(reader: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    reader.read(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn size() -> PtySize {
        PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    #[test]
    fn echo_runs_on_a_pty_and_output_is_readable() {
        let launch = Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            cwd: None,
        };
        let mut process = PtyProcess::spawn(&launch, size()).expect("spawn echo on pty");
        let mut reader = process.take_reader().expect("reader");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 256];
        while Instant::now() < deadline && !String::from_utf8_lossy(&output).contains("hi") {
            match read_chunk(&mut reader, &mut buffer) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(_) => break,
            }
        }
        assert!(
            String::from_utf8_lossy(&output).contains("hi"),
            "pty output: {output:?}"
        );
    }

    #[test]
    fn the_child_does_not_inherit_the_master() {
        let script = "for fd in /proc/$$/fd/*; do readlink \"$fd\"; done; echo done";
        let launch = Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: None,
        };
        let mut process = PtyProcess::spawn(&launch, size()).expect("spawn fd listing on pty");
        let mut reader = process.take_reader().expect("reader");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 1024];
        while Instant::now() < deadline && !String::from_utf8_lossy(&output).contains("done") {
            match read_chunk(&mut reader, &mut buffer) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(_) => break,
            }
        }
        let listing = String::from_utf8_lossy(&output);
        assert!(listing.contains("done"), "fd listing: {listing}");
        assert!(
            !listing.contains("ptmx"),
            "child holds the pty master: {listing}"
        );
    }

    /// Re-runs the inner test in a process that ignores SIGINT and SIGQUIT, as a terminal started in the background
    /// by a shell script is.
    #[test]
    fn ctrl_c_interrupts_the_child_of_a_terminal_launched_ignoring_sigint() {
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new("/bin/sh")
            .args([
                "-c",
                "trap '' INT QUIT; exec \"$0\" --exact --ignored --test-threads=1 \
                 pty::tests::ctrl_c_interrupts_the_foreground_child",
            ])
            .arg(exe)
            .stdout(Stdio::null())
            .status()
            .expect("run the inner test");
        assert!(status.success(), "Ctrl+C did not interrupt the child");
    }

    #[test]
    #[ignore = "run by ctrl_c_interrupts_the_child_of_a_terminal_launched_ignoring_sigint"]
    fn ctrl_c_interrupts_the_foreground_child() {
        use std::os::unix::process::ExitStatusExt;
        let launch = Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo ready; exec sleep 30".into()],
            cwd: None,
        };
        let mut process = PtyProcess::spawn(&launch, size()).expect("spawn sleep on pty");
        let mut reader = process.take_reader().expect("reader");
        let mut output = Vec::new();
        let mut buffer = [0_u8; 256];
        while !String::from_utf8_lossy(&output).contains("ready") {
            match read_chunk(&mut reader, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("ready"));
        std::thread::sleep(Duration::from_millis(200));
        process.write(vec![0x03]).expect("queue Ctrl+C");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = process.child.try_wait().expect("child status") {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status.and_then(|status| status.signal()),
            Some(2),
            "the foreground child ends on SIGINT"
        );
    }

    #[test]
    fn launch_shell_defaults_to_env_or_sh() {
        let launch = Launch::shell(None);
        assert_ne!(launch.program, "");
        assert_eq!(launch.args, vec!["-i"]);
    }

    #[test]
    fn closing_hangs_up_a_shell_that_saves_state_and_still_reclaims_one_that_does_not() {
        use std::os::unix::process::ExitStatusExt;
        // A polite shell saves its file in the SIGHUP trap and exits cleanly.
        let marker = std::env::temp_dir().join(format!("flamewm-pty-hup-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let mut polite = Command::new("/bin/sh")
            .args([
                "-c",
                "trap 'echo saved > \"$0\"; exit 0' HUP; while :; do sleep 0.05; done",
            ])
            .arg(&marker)
            .spawn()
            .expect("spawn polite shell");
        std::thread::sleep(Duration::from_millis(50));
        let status = hangup_then_reap(&mut polite).expect("reap the polite shell");
        assert!(status.success(), "the polite shell exits from its HUP trap");
        assert_eq!(
            std::fs::read_to_string(&marker)
                .expect("state written on hangup")
                .trim(),
            "saved"
        );
        let _ = std::fs::remove_file(&marker);
        // A child that ignores the hangup is still gone before the master closes.
        let mut stubborn = Command::new("/bin/sh")
            .args(["-c", "trap '' HUP; while :; do sleep 0.05; done"])
            .spawn()
            .expect("spawn stubborn shell");
        std::thread::sleep(Duration::from_millis(50));
        let status = hangup_then_reap(&mut stubborn).expect("reap the stubborn shell");
        assert_eq!(
            status.signal(),
            Some(9),
            "a hung-up child is killed after the grace"
        );
    }
}
