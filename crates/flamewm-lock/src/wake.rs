//! One cheap cross-thread wake for the lock loop: a byte on the socket pair its
//! reactor watches. Worker threads use it so a result is applied at once instead of
//! waiting for the next idle tick, which keeps the tick interval free to grow.
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

/// Clones share one socket. Every wake is a single byte; a full socket already has a
/// wake pending and a closed one means the loop is gone, so `wake` never blocks or fails.
#[derive(Clone)]
pub struct Waker(Arc<UnixStream>);

impl Waker {
    /// Wraps the loop's wake socket. The caller makes it nonblocking; a blocked write
    /// would stall the worker that only wants to nudge the loop.
    #[must_use]
    pub fn new(socket: UnixStream) -> Self {
        Self(Arc::new(socket))
    }

    /// Wakes the lock loop out of `poll`. Returns `false` only when the loop side is
    /// closed, which lets the frame pacer thread exit with the lock. Any other error is
    /// ignored: the tick covers a lost wake.
    #[must_use]
    pub fn wake(&self) -> bool {
        // `&UnixStream` writes directly; the socket is nonblocking, so a full socket just
        // means a wake is already pending.
        match (&*self.0).write_all(&[1]) {
            Ok(()) => true,
            Err(error) => error.kind() != std::io::ErrorKind::BrokenPipe,
        }
    }
}
