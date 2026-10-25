//! PTY ownership: master/slave pair (rustix, safe API), child spawn on the slave, a writer thread so key input never
//! blocks the UI, and TIOCSWINSZ on resize. The child acquires the slave as its controlling terminal through
//! util-linux `setsid -c`, so Ctrl+C and job control work without raw FFI in this crate.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
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
    pub fn shell(cwd: Option<std::path::PathBuf>) -> Self {
        let program = std::env::var("SHELL").ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| "/bin/sh".into());
        Self { program, args: Vec::new(), cwd }
    }
}

impl PtyProcess {
    /// Opens a PTY pair, sizes it, and starts `launch` on the slave with a controlling terminal when `setsid` exists.
    pub fn spawn(launch: &Launch, size: PtySize) -> io::Result<Self> {
        // Close-on-exec: a child holding the master never sees the terminal hang up, so closing the window would
        // leave the shell and everything it started running.
        let master: OwnedFd = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
        grantpt(&master)?;
        unlockpt(&master)?;
        let slave_name = ptsname(&master, Vec::new())?;
        let slave_path = Path::new(std::ffi::OsStr::new(slave_name.to_str().map_err(|error| io::Error::other(error.to_string()))?));
        tcsetwinsize(&master, winsize(size))?;
        let master = File::from(master);
        let slave = OpenOptions::new().read(true).write(true).open(slave_path)?;
        let child = spawn_child(launch, &slave)?;
        drop(slave);
        let reader = master.try_clone()?;
        let mut writable = master.try_clone()?;
        let (writer, queue) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("flamewm-terminal-write".into())
            .spawn(move || {
                for bytes in queue {
                    if writable.write_all(&bytes).and_then(|()| writable.flush()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { child, master, writer, reader: Some(reader) })
    }

    /// Whether the line discipline is canonical. Shell line editors clear ICANON while they read keys at the prompt, so
    /// a set flag means a program reads cooked lines (sudo, cat, a REPL) rather than the prompt. Errors count as canonical.
    pub fn canonical(&self) -> bool {
        rustix::termios::tcgetattr(&self.master).map_or(true, |attrs| attrs.local_modes.contains(rustix::termios::LocalModes::ICANON))
    }

    /// Whether the terminal echoes typed input. A shell line editor turns echo off and draws the line itself, while
    /// a password prompt also turns it off; the prompt check therefore needs both flags.
    pub fn echoing(&self) -> bool {
        rustix::termios::tcgetattr(&self.master).map_or(true, |attrs| attrs.local_modes.contains(rustix::termios::LocalModes::ECHO))
    }

    /// The master read side, owned by the reader thread. Returns `None` after the first call.
    pub fn take_reader(&mut self) -> Option<File> {
        self.reader.take()
    }

    /// Queues bytes for the child; the writer thread performs the blocking write.
    pub fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.writer.send(bytes).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "terminal writer stopped"))
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        tcsetwinsize(&self.master, winsize(size)).map_err(io::Error::from)
    }

    /// The kernel's window size for this terminal, which is what the child sees.
    pub fn winsize(&self) -> io::Result<PtySize> {
        let size = rustix::termios::tcgetwinsize(&self.master)?;
        Ok(PtySize { cols: size.ws_col, rows: size.ws_row, pixel_width: size.ws_xpixel, pixel_height: size.ws_ypixel })
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

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
    Winsize { ws_row: size.rows, ws_col: size.cols, ws_xpixel: size.pixel_width, ws_ypixel: size.pixel_height }
}

fn spawn_child(launch: &Launch, slave: &File) -> io::Result<Child> {
    let env = |command: &mut Command| {
        command.env("TERM", "xterm-256color").env("COLORTERM", "truecolor").env("TERM_PROGRAM", "FlameWM");
        if let Some(cwd) = &launch.cwd {
            command.current_dir(cwd);
        }
    };
    let stdio = |command: &mut Command| -> io::Result<()> {
        command.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave.try_clone()?));
        Ok(())
    };
    // `setsid -c` makes the slave the controlling terminal; without util-linux the shell still runs, without job control.
    let mut wrapped = Command::new("setsid");
    wrapped.arg("-c").arg(&launch.program).args(&launch.args);
    env(&mut wrapped);
    stdio(&mut wrapped)?;
    match wrapped.spawn() {
        Ok(child) => Ok(child),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut direct = Command::new(&launch.program);
            direct.args(&launch.args);
            env(&mut direct);
            stdio(&mut direct)?;
            direct.spawn()
        }
        Err(error) => Err(error),
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
        PtySize { cols: 80, rows: 24, pixel_width: 0, pixel_height: 0 }
    }

    #[test]
    fn echo_runs_on_a_pty_and_output_is_readable() {
        let launch = Launch { program: "/bin/sh".into(), args: vec!["-c".into(), "echo hi".into()], cwd: None };
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
        assert!(String::from_utf8_lossy(&output).contains("hi"), "pty output: {output:?}");
    }

    #[test]
    fn the_child_does_not_inherit_the_master() {
        let script = "for fd in /proc/$$/fd/*; do readlink \"$fd\"; done; echo done";
        let launch = Launch { program: "/bin/sh".into(), args: vec!["-c".into(), script.into()], cwd: None };
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
        assert!(!listing.contains("ptmx"), "child holds the pty master: {listing}");
    }

    #[test]
    fn launch_shell_defaults_to_env_or_sh() {
        let launch = Launch::shell(None);
        assert!(!launch.program.is_empty());
        assert!(launch.args.is_empty());
    }
}
