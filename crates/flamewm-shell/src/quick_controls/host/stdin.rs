use super::*;

/// Duplicate stdin + `O_NONBLOCK` on the dup. No unsafe: the std File
/// duplicate path (`try_clone`) keeps fd 0 stdio-owned while the clone
/// gets its own description flags via `set_nonblocking`.
pub(crate) fn stdin_nonblocking_dup() -> Result<std::fs::File, HostError> {
    let file = std::fs::File::open("/dev/stdin").map_err(HostError::Io)?;
    let flags = rustix::fs::fcntl_getfl(&file)
        .map_err(|error| HostError::Io(io::Error::from_raw_os_error(error.raw_os_error())))?;
    rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| HostError::Io(io::Error::from_raw_os_error(error.raw_os_error())))?;
    Ok(file)
}

/// Shared parent-input state: partial line bytes + decoded queue live
/// behind one mutex shared by the stdin FD callback and the tick.
#[derive(Default)]
pub(crate) struct ParentInput {
    pub(crate) pending: Vec<u8>,
    pub(crate) queue: VecDeque<Command>,
    pub(crate) eof: bool,
}

pub(crate) fn push_line(state: &mut ParentInput, line: &[u8]) {
    if line.len() > MAX_LINE_LEN {
        eprintln!("flamewm-quick-control: overlong command line dropped");
        return;
    }
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    match decode_line(trimmed) {
        Ok(command) => state.queue.push_back(command),
        Err(error) => eprintln!("flamewm-quick-control: bad command: {error}"),
    }
}

/// Drain available bytes from the nonblocking parent FD: accumulate
/// partial lines, newline parses to queue, EOF (Ok(0)) shuts down.
pub(crate) fn drain_parent_fd(file: &mut std::fs::File, state: &Arc<Mutex<ParentInput>>) {
    let mut buf = [0u8; 4096];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                if let Ok(mut guard) = state.lock() {
                    guard.eof = true;
                }
                return;
            }
            Ok(n) => {
                let Ok(mut guard) = state.lock() else { return };
                let mut start = 0usize;
                while start < n {
                    if let Some(rel) = buf[start..n].iter().position(|b| *b == b'\n') {
                        let end = start + rel;
                        let mut line = core::mem::take(&mut guard.pending);
                        line.extend_from_slice(&buf[start..end]);
                        // Strip a trailing CR so CRLF parents stay total.
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        push_line(&mut guard, &line);
                        start = end + 1;
                    } else {
                        guard.pending.extend_from_slice(&buf[start..n]);
                        if guard.pending.len() > MAX_LINE_LEN {
                            guard.pending.clear();
                            eprintln!("flamewm-quick-control: overlong command line dropped");
                        }
                        break;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                if let Ok(mut guard) = state.lock() {
                    guard.eof = true;
                }
                return;
            }
        }
    }
}

/// Packaged data root the desktop layer resolves the default wallpaper against.
pub(crate) static PACKAGED_ROOT: std::sync::OnceLock<std::path::PathBuf> =
    std::sync::OnceLock::new();
