//! Child-process stdio as a reactor source. The source owns the `Child`, its stdin and its stdout;
//! the reactor owns the registration. Dropping the source closes both pipes; the caller reaps the
//! child with [`ChildPipe::wait`] or [`ChildPipe::kill`].

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

pub struct ChildPipe {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
}

impl ChildPipe {
    /// Spawns `command` with piped stdin and stdout. Stdout is nonblocking for reactor reads.
    ///
    /// # Errors
    /// Returns the spawn error, or an error when the pipes cannot be set up.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("child stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout missing"))?;
        let flags = fcntl_getfl(stdout.as_fd()).map_err(io::Error::from)?;
        fcntl_setfl(stdout.as_fd(), flags | OFlags::NONBLOCK).map_err(io::Error::from)?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout,
        })
    }

    /// Writes `line` plus a newline to the child's stdin.
    ///
    /// # Errors
    /// Returns `BrokenPipe` after [`close_stdin`](Self::close_stdin), or the write error.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "child stdin closed"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// Closes the child's stdin, signalling end of input.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Reads what is available from the child's stdout. `Ok(0)` is end of file;
    /// `WouldBlock` means nothing is ready yet.
    ///
    /// # Errors
    /// Returns the read error.
    pub fn read_some(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stdout.read(buf)
    }

    /// Kills the child if it is still running.
    ///
    /// # Errors
    /// Returns the kill error.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    /// Closes stdin and waits for the child to exit.
    ///
    /// # Errors
    /// Returns the wait error.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.close_stdin();
        self.child.wait()
    }
}

impl AsFd for ChildPipe {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stdout.as_fd()
    }
}

impl AsRawFd for ChildPipe {
    fn as_raw_fd(&self) -> RawFd {
        self.stdout.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{FdAction, Reactor};

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[test]
    fn stdin_line_round_trips_through_child_stdout() {
        let mut pipe = ChildPipe::spawn(&mut sh("read line; echo \"got $line\"")).expect("spawn");
        pipe.write_line("secret").expect("write");
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = [0u8; 64];
        loop {
            match pipe.read_some(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "child reply timed out");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("read: {e}"),
            }
        }
        assert_eq!(String::from_utf8_lossy(&out), "got secret\n");
        assert!(pipe.wait().expect("wait").success());
    }

    #[test]
    fn write_after_close_stdin_is_broken_pipe() {
        let mut pipe = ChildPipe::spawn(&mut sh("cat >/dev/null")).expect("spawn");
        pipe.close_stdin();
        let err = pipe.write_line("x").expect_err("closed");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        pipe.wait().expect("wait");
    }

    #[test]
    fn reactor_dispatches_child_output_and_removes_source_at_eof() {
        let mut reactor = Reactor::new().expect("reactor");
        let pipe = ChildPipe::spawn(&mut sh("printf 'PAM_PROMPT_ECHO_OFF x\\n'")).expect("spawn");
        let seen = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&seen);
        reactor
            .register_child_pipe(pipe, move |_fd, pipe, _readiness| {
                let mut buf = [0u8; 128];
                match pipe.read_some(&mut buf) {
                    Ok(0) => FdAction::Remove,
                    Ok(n) => {
                        sink.borrow_mut()
                            .push_str(&String::from_utf8_lossy(&buf[..n]));
                        FdAction::Continue
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => FdAction::Continue,
                    Err(_) => FdAction::Remove,
                }
            })
            .expect("register");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !seen.borrow().ends_with('\n') && Instant::now() < deadline {
            reactor
                .dispatch(Some(Duration::from_millis(20)))
                .expect("dispatch");
        }
        assert_eq!(seen.borrow().as_str(), "PAM_PROMPT_ECHO_OFF x\n");
    }
}
