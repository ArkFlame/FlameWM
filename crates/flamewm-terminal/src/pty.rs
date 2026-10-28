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

    /// Whether the line discipline is canonical. Shell line editors clear ICANON while they read keys at the prompt, so
    /// a set flag means a program reads cooked lines (sudo, cat, a REPL) rather than the prompt. Errors count as canonical.
    #[must_use]
    pub fn canonical(&self) -> bool {
        rustix::termios::tcgetattr(&self.master).map_or(true, |attrs| {
            attrs
                .local_modes
                .contains(rustix::termios::LocalModes::ICANON)
        })
    }

    /// Whether the terminal echoes typed input. A shell line editor turns echo off and draws the line itself, while
    /// a password prompt also turns it off; the prompt check therefore needs both flags.
    #[must_use]
    pub fn echoing(&self) -> bool {
        rustix::termios::tcgetattr(&self.master).map_or(true, |attrs| {
            attrs
                .local_modes
                .contains(rustix::termios::LocalModes::ECHO)
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

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
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
        assert!(!launch.program.is_empty());
        assert_eq!(launch.args, vec!["-i"]);
    }
}
