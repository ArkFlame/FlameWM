//! Parent-facing supervisor for the standalone quick-control host.
//!
//! Owns a [`std::process::Child`], polls with `try_wait` only (never blocks),
//! bounds restarts, reaps on poll (no zombies), and shuts down idempotently.

use std::{
    collections::{BTreeSet, VecDeque},
    io::Read,
    process::Child,
};

use super::protocol::{encode_command, Command, OpenRequest, OpenResult, OpenScope};
use flamewm_shell_core::weather::WeatherFeed;

pub const DEFAULT_MAX_RESTARTS: u32 = 3;

#[derive(Debug)]
pub enum SupervisorError {
    Io(std::io::Error),
    NotRunning,
    RestartBudgetExhausted,
    WouldBlock,
}

impl core::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "supervisor i/o: {e}"),
            Self::NotRunning => write!(f, "quick-control host not running"),
            Self::RestartBudgetExhausted => write!(f, "restart budget exhausted"),
            Self::WouldBlock => write!(f, "quick-control host stdin backpressure"),
        }
    }
}

impl std::error::Error for SupervisorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::NotRunning | Self::RestartBudgetExhausted | Self::WouldBlock => None,
        }
    }
}

impl From<std::io::Error> for SupervisorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Pure seam: classify a write failure. `WouldBlock` becomes
/// backpressure (nonfatal); anything else stays an I/O error.
fn classify_write_error(error: std::io::Error) -> SupervisorError {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        SupervisorError::WouldBlock
    } else {
        SupervisorError::Io(error)
    }
}

/// Pure seam: CLOSE/SHUTDOWN swallow `WouldBlock` (best-effort).
fn close_result(result: Result<(), SupervisorError>) -> Result<(), SupervisorError> {
    match result {
        Err(SupervisorError::WouldBlock) => Ok(()),
        other => other,
    }
}

/// Pure seam: OPEN coalesces to newest — a stalled request is replaced.
fn coalesce_pending_open(slot: &mut Option<OpenRequest>, req: OpenRequest) {
    *slot = Some(req);
}
fn set_stdin_nonblocking(child: &mut Child) -> Result<(), SupervisorError> {
    use std::os::unix::io::AsFd;
    let stdin = child.stdin.as_mut().ok_or(SupervisorError::NotRunning)?;
    let io = |e: rustix::io::Errno| SupervisorError::Io(e.into());
    let flags = rustix::fs::fcntl_getfl(stdin.as_fd()).map_err(io)?;
    rustix::fs::fcntl_setfl(stdin.as_fd(), flags | rustix::fs::OFlags::NONBLOCK).map_err(io)?;
    Ok(())
}

fn set_stdout_nonblocking(child: &mut Child) -> Result<(), SupervisorError> {
    use std::os::unix::io::AsFd;
    let stdout = child.stdout.as_mut().ok_or(SupervisorError::NotRunning)?;
    let io = |e: rustix::io::Errno| SupervisorError::Io(e.into());
    let flags = rustix::fs::fcntl_getfl(stdout.as_fd()).map_err(io)?;
    rustix::fs::fcntl_setfl(stdout.as_fd(), flags | rustix::fs::OFlags::NONBLOCK).map_err(io)?;
    Ok(())
}

fn spawn_host(program: &str) -> Result<Child, SupervisorError> {
    let mut child = std::process::Command::new(program)
        .arg("--quick-control-host")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    set_stdin_nonblocking(&mut child)?;
    set_stdout_nonblocking(&mut child)?;
    Ok(child)
}

pub struct QuickControlSupervisor {
    child: Option<Child>,
    program: String,
    restarts: u32,
    max_restarts: u32,
    shut_down: bool,
    pending: Option<OpenRequest>,
    response_bytes: Vec<u8>,
    open_results: VecDeque<OpenResult>,
    awaiting: BTreeSet<u64>,
}

impl QuickControlSupervisor {
    #[must_use]
    pub fn new(program: String) -> Self {
        Self::with_max_restarts(program, DEFAULT_MAX_RESTARTS)
    }

    #[must_use]
    pub fn with_max_restarts(program: String, max_restarts: u32) -> Self {
        Self {
            child: None,
            program,
            restarts: 0,
            max_restarts,
            shut_down: false,
            pending: None,
            response_bytes: Vec::new(),
            open_results: VecDeque::new(),
            awaiting: BTreeSet::new(),
        }
    }

    /// Spawn the host after the panel is up. Marks child stdin
    /// `O_NONBLOCK` (rustix) so parent writes never block the shell loop;
    /// a full pipe surfaces as `WouldBlock` backpressure (nonfatal).
    pub fn start_after_panel(&mut self) -> Result<(), SupervisorError> {
        if self.child.is_some() {
            return Ok(());
        }
        self.shut_down = false;
        self.child = Some(spawn_host(&self.program)?);
        Ok(())
    }

    /// Nonblocking write of one encoded line. `WouldBlock` maps to
    /// `SupervisorError::WouldBlock` (nonfatal backpressure), never blocks.
    fn send(&mut self, cmd: &Command) -> Result<(), SupervisorError> {
        use std::io::Write;
        let child = self.child.as_mut().ok_or(SupervisorError::NotRunning)?;
        let stdin = child.stdin.as_mut().ok_or(SupervisorError::NotRunning)?;
        stdin
            .write_all(format!("{}\n", encode_command(cmd)).as_bytes())
            .map_err(classify_write_error)
    }

    /// Pushes the weather feed to the popup host over the same stdin line channel as OPEN.
    /// `WouldBlock` is nonfatal backpressure; the caller may retry on a later tick.
    pub fn push_weather(&mut self, feed: WeatherFeed) -> Result<(), SupervisorError> {
        self.send(&Command::Weather(feed))
    }

    /// OPEN coalesces to newest: the newest request replaces any stalled
    /// one; no synchronous retry after `WouldBlock`, the next tick's newest
    /// state wins (caller passes latest state).
    pub fn open(&mut self, req: OpenRequest) -> Result<(), SupervisorError> {
        coalesce_pending_open(&mut self.pending, req.clone());
        let result = self.send(&Command::Open(req));
        if result.is_ok() {
            if let Some(scope) = self
                .pending
                .as_ref()
                .and_then(|request| request.scope.as_ref())
            {
                self.awaiting.insert(scope.sequence);
            }
            self.pending = None;
        }
        result
    }

    pub fn open_scoped(
        &mut self,
        mut req: OpenRequest,
        scope: OpenScope,
    ) -> Result<(), SupervisorError> {
        req.scope = Some(scope);
        self.open(req)
    }

    /// CLOSE/SHUTDOWN are best-effort: `WouldBlock` is swallowed so a stalled
    /// pipe never blocks shutdown paths.
    pub fn close(&mut self) -> Result<(), SupervisorError> {
        close_result(self.send(&Command::Close))
    }

    pub fn close_session(&mut self, sequence: u64) -> Result<(), SupervisorError> {
        close_result(self.send(&Command::CloseSession(sequence)))
    }

    fn drain_open_responses(&mut self) -> Result<(), SupervisorError> {
        use std::io::ErrorKind;
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let Some(stdout) = child.stdout.as_mut() else {
            return Err(SupervisorError::NotRunning);
        };
        let mut buffer = [0u8; 1024];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    self.response_bytes.extend_from_slice(&buffer[..n]);
                    while let Some(end) = self.response_bytes.iter().position(|byte| *byte == b'\n')
                    {
                        let line = self.response_bytes.drain(..=end).collect::<Vec<_>>();
                        if let Ok(line) = std::str::from_utf8(&line[..line.len() - 1]) {
                            if let Ok(result) = OpenResult::decode(line.trim()) {
                                if let Some(sequence) = open_result_sequence(result) {
                                    self.awaiting.remove(&sequence);
                                }
                                self.open_results.push_back(result);
                            }
                        }
                    }
                    if self.response_bytes.len() > super::protocol::MAX_LINE_LEN {
                        self.response_bytes.clear();
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(classify_write_error(error)),
            }
        }
        Ok(())
    }

    /// Read available helper responses without blocking. Child exit resolves
    /// every still outstanding OPEN so callers can release its popup lease.
    pub fn take_open_results(&mut self) -> Result<Vec<OpenResult>, SupervisorError> {
        self.drain_open_responses()?;
        Ok(self.open_results.drain(..).collect())
    }

    /// Non-blocking poll: reaps an exited child (`try_wait`), counts one
    /// restart while budget remains, and never treats a nonzero exit as
    /// fatal by itself. Returns the exit status when the child exited.
    pub fn poll(&mut self) -> Result<Option<std::process::ExitStatus>, SupervisorError> {
        self.drain_open_responses()?;
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let Some(status) = child.try_wait()? else {
            return Ok(None);
        };
        // Reaped via try_wait above; drop handle so no zombie lingers.
        self.child = None;
        self.response_bytes.clear();
        self.open_results.extend(
            std::mem::take(&mut self.awaiting)
                .into_iter()
                .map(OpenResult::HostExited),
        );
        if self.shut_down {
            return Ok(Some(status));
        }
        if self.restarts >= self.max_restarts {
            return Err(SupervisorError::RestartBudgetExhausted);
        }
        self.restarts += 1;
        self.child = Some(spawn_host(&self.program)?);
        Ok(Some(status))
    }

    /// Idempotent shutdown: best-effort Shutdown line + kill, then reap.
    /// Safe to call repeatedly or when never started.
    pub fn shutdown(&mut self) -> Result<(), SupervisorError> {
        self.shut_down = true;
        if let Some(mut child) = self.child.take() {
            let _ = self.send_to(&mut child, &Command::Shutdown);
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn send_to(&self, child: &mut Child, cmd: &Command) -> Result<(), SupervisorError> {
        use std::io::Write;
        // Best-effort shutdown path: never block, never fail.
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(format!("{}\n", encode_command(cmd)).as_bytes());
        }
        Ok(())
    }

    #[must_use]
    pub fn restart_count(&self) -> u32 {
        self.restarts
    }
}

fn open_result_sequence(result: OpenResult) -> Option<u64> {
    match result {
        OpenResult::Presented(sequence)
        | OpenResult::Refused(sequence)
        | OpenResult::RolledBack(sequence)
        | OpenResult::HostExited(sequence) => Some(sequence),
    }
}

impl Drop for QuickControlSupervisor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick_controls::protocol::QuickControlKind;
    use flamewm_api::{PanelEdge, Rect};

    fn req() -> OpenRequest {
        OpenRequest {
            kind: QuickControlKind::Audio,
            anchor: Rect::new(0, 0, 10, 10),
            work_area: Rect::new(0, 0, 100, 100),
            panel_edge: PanelEdge::Bottom,
            dock: None,
            scope: None,
        }
    }

    #[test]
    fn helper_death_resolves_unacknowledged_scoped_open() {
        use flamewm_api::OutputId;
        let mut sup = QuickControlSupervisor::with_max_restarts("false".to_owned(), 0);
        let mut child = std::process::Command::new("sh")
            .args(["-c", "IFS= read -r _; exit 0"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn helper fixture");
        set_stdin_nonblocking(&mut child).expect("nonblocking stdin");
        set_stdout_nonblocking(&mut child).expect("nonblocking stdout");
        sup.child = Some(child);
        let sequence = 55;
        sup.open_scoped(
            req(),
            OpenScope {
                output: OutputId::new("A"),
                topology_generation: 3,
                sequence,
            },
        )
        .expect("send scoped open");
        let mut exited = false;
        for _ in 0..100 {
            match sup.poll() {
                Ok(Some(_)) | Err(SupervisorError::RestartBudgetExhausted) => {
                    exited = true;
                    break;
                }
                Ok(None) => {}
                Err(error) => panic!("poll helper: {error}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(exited, "helper fixture should exit after receiving OPEN");
        assert_eq!(
            sup.take_open_results().expect("drain helper outcomes"),
            vec![OpenResult::HostExited(sequence)]
        );
        sup.shutdown().expect("shutdown restarted child");
    }

    #[test]
    fn fake_child_nonzero_exit_is_nonfatal() {
        // `false` exits 1 immediately: poll reaps it and restarts.
        let mut sup = QuickControlSupervisor::with_max_restarts("false".to_owned(), 5);
        sup.start_after_panel().expect("spawn false");
        // Wait briefly for the trivial child to exit.
        for _ in 0..50 {
            if sup.poll().is_ok() && sup.restart_count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(sup.restart_count() > 0, "expected a restart after exit 1");
        sup.shutdown().expect("shutdown");
    }

    #[test]
    fn restart_budget_is_bounded() {
        let mut sup = QuickControlSupervisor::with_max_restarts("false".to_owned(), 1);
        sup.start_after_panel().expect("spawn");
        let mut exhausted = false;
        for _ in 0..400 {
            match sup.poll() {
                Err(SupervisorError::RestartBudgetExhausted) => {
                    exhausted = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(exhausted, "expected bounded restart budget to exhaust");
        assert!(sup.restart_count() <= 1);
    }

    #[test]
    fn shutdown_is_idempotent_and_open_after_shutdown_fails_cleanly() {
        let mut sup = QuickControlSupervisor::new("false".to_owned());
        // Never started: still Ok.
        sup.shutdown().expect("first shutdown");
        sup.shutdown().expect("second shutdown");
        assert!(matches!(sup.open(req()), Err(SupervisorError::NotRunning)));
        assert!(matches!(sup.poll(), Ok(None)));
    }

    /// Supervisor whose child is `sleep 30`: holds the stdin read end open
    /// without draining, so writes hit backpressure.
    fn stalled_supervisor() -> QuickControlSupervisor {
        let mut sup = QuickControlSupervisor::new("sleep".to_owned());
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sleep");
        set_stdin_nonblocking(&mut child).expect("nonblocking stdin");
        sup.child = Some(child);
        sup
    }

    #[test]
    fn full_stdin_pipe_is_nonblocking_wouldblock_immediate() {
        use std::io::Write;
        // Writes must return WouldBlock instead of blocking the parent.
        let mut sup = stalled_supervisor();
        let stdin = sup.child.as_mut().unwrap().stdin.as_mut().unwrap();
        // Fill the pipe until the kernel refuses; must stay nonblocking.
        let start = std::time::Instant::now();
        let chunk = vec![0u8; 65536];
        let mut saw_wouldblock = false;
        for _ in 0..256 {
            match stdin.write(&chunk) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    saw_wouldblock = true;
                    break;
                }
                Err(e) => panic!("unexpected write error: {e}"),
            }
            if start.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
        }
        assert!(saw_wouldblock, "expected pipe to exert backpressure");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "write must be immediate, not blocked"
        );
        // A new OPEN after the stall is still nonblocking (WouldBlock, not hang).
        let open_start = std::time::Instant::now();
        let result = sup.open(req());
        assert!(matches!(result, Err(SupervisorError::WouldBlock)));
        assert!(open_start.elapsed() < std::time::Duration::from_secs(2));
        // CLOSE/SHUTDOWN lines are best-effort: WouldBlock swallowed.
        assert!(sup.close().is_ok());
        sup.shutdown().expect("shutdown");
    }

    #[test]
    fn open_after_stall_stays_nonblocking() {
        use std::io::Write;
        // Direct WouldBlock mapping: a second OPEN after backpressure
        // returns immediately rather than blocking the shell loop.
        let mut sup = stalled_supervisor();
        let chunk = vec![0u8; 65536];
        for _ in 0..256 {
            let stdin = sup.child.as_mut().unwrap().stdin.as_mut().unwrap();
            if stdin.write(&chunk).is_err() {
                break;
            }
        }
        let start = std::time::Instant::now();
        let _ = sup.open(req());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        sup.shutdown().expect("shutdown");
    }

    #[test]
    fn pure_seams_cover_backpressure_coalesce_best_effort() {
        // classify_write_error: WouldBlock is nonfatal backpressure.
        let wb = classify_write_error(std::io::Error::from(std::io::ErrorKind::WouldBlock));
        assert!(matches!(wb, SupervisorError::WouldBlock));
        let io = classify_write_error(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(matches!(io, SupervisorError::Io(_)));
        // close_result: WouldBlock swallowed, real I/O still fatal.
        assert!(close_result(Err(SupervisorError::WouldBlock)).is_ok());
        assert!(close_result(Ok(())).is_ok());
        // coalesce_pending_open: newest OPEN replaces the stalled one.
        let mut slot: Option<OpenRequest> = None;
        coalesce_pending_open(&mut slot, req());
        assert!(slot.is_some());
        let mut newer = req();
        newer.anchor = Rect::new(5, 5, 10, 10);
        coalesce_pending_open(&mut slot, newer);
        assert_eq!(slot.unwrap().anchor.x, 5);
    }
}
