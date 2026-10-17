//! Small calloop-backed reactor for FlameWM's single desktop event loop.
//!
//! Registered I/O objects are transferred to calloop and are released with the registration; this
//! crate never duplicates a descriptor. Tokio is deliberately not part of this integration boundary.

use std::cell::Cell;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::panic::Location;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, Mode, PostAction, Readiness, RegistrationToken};

pub mod timeline;
pub use timeline::{Easing, Start, Timeline, TransitionId};

/// Readiness flags exposed by the reactor without leaking calloop's source types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FdReadiness {
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub hangup: bool,
}

/// Action returned by an FD callback after handling readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdAction {
    /// Keep the source registered for later readiness notifications.
    Continue,
    /// Remove the source and drop its owned descriptor.
    Remove,
}

impl From<Readiness> for FdReadiness {
    fn from(readiness: Readiness) -> Self {
        Self {
            readable: readiness.readable,
            writable: readiness.writable,
            error: readiness.error,
            // calloop reports terminal/error readiness but does not distinguish HUP from ERR.
            hangup: readiness.error,
        }
    }
}

/// The process-local event loop owner.
pub struct Reactor {
    event_loop: EventLoop<'static, ()>,
    wake: WakeDeadline,
}

/// Shared wake deadline: dispatch never sleeps past it. Owners set it from their tick without
/// reactor access. `None` leaves dispatch bounded only by its own timeout (zero idle wakeups).
#[derive(Clone, Debug, Default)]
pub struct WakeDeadline(Rc<Cell<Option<Instant>>>);

impl WakeDeadline {
    pub fn set(&self, at: Option<Instant>) {
        self.0.set(at);
    }

    #[must_use]
    pub fn get(&self) -> Option<Instant> {
        self.0.get()
    }
}

/// Dispatch timeout bounded by a pending wake deadline. Pure for unit tests.
#[must_use]
pub fn bounded_timeout(
    timeout: Option<Duration>,
    deadline: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    let Some(at) = deadline else {
        return timeout;
    };
    let until = at.saturating_duration_since(now);
    Some(timeout.map_or(until, |timeout| timeout.min(until)))
}

struct BorrowedRawFd(RawFd);

impl AsFd for BorrowedRawFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // The caller keeps this descriptor alive for the registration lifetime.
        #[allow(unsafe_code)]
        unsafe {
            BorrowedFd::borrow_raw(self.0)
        }
    }
}

impl AsRawFd for BorrowedRawFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Reactor {
    /// Creates an empty reactor.
    pub fn new() -> calloop::Result<Self> {
        Ok(Self {
            event_loop: EventLoop::try_new()?,
            wake: WakeDeadline::default(),
        })
    }

    /// Handle to the shared wake deadline consumed by [`Reactor::dispatch`].
    #[must_use]
    pub fn wake_deadline(&self) -> WakeDeadline {
        self.wake.clone()
    }

    /// Registers an `AsFd` source. The source is owned by calloop until removal or shutdown.
    pub fn register_fd<F, C>(
        &mut self,
        source: F,
        interest: Interest,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        F: AsFd + AsRawFd + 'static,
        C: FnMut(i32, FdReadiness) + 'static,
    {
        let mut callback = callback;
        self.register_fd_with_action(source, interest, move |fd, readiness| {
            callback(fd, readiness);
            FdAction::Continue
        })
    }

    /// Registers an `AsFd` source whose callback controls its registration lifecycle.
    ///
    /// Returning [`FdAction::Remove`] unregisters the source immediately after the callback and
    /// drops the source. [`Reactor::remove`] remains available for explicit token-based removal.
    pub fn register_fd_with_action<F, C>(
        &mut self,
        source: F,
        interest: Interest,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        F: AsFd + AsRawFd + 'static,
        C: FnMut(i32, FdReadiness) -> FdAction + 'static,
    {
        let fd = source.as_raw_fd();
        let mut callback = callback;
        self.event_loop
            .handle()
            .insert_source(
                Generic::new(source, interest, Mode::Level),
                move |readiness, _, _| {
                    let action = callback(fd, readiness.into());
                    Ok(match action {
                        FdAction::Continue => PostAction::Continue,
                        FdAction::Remove => PostAction::Remove,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Registers an owned source and exposes it to the readiness callback for nonblocking reads.
    pub fn register_fd_with_source_action<F, C>(
        &mut self,
        source: F,
        interest: Interest,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        F: AsFd + AsRawFd + 'static,
        C: FnMut(i32, &mut F, FdReadiness) -> FdAction + 'static,
    {
        let fd = source.as_raw_fd();
        let mut callback = callback;
        self.event_loop
            .handle()
            .insert_source(
                Generic::new(source, interest, Mode::Level),
                move |readiness, source, _| {
                    #[allow(unsafe_code)]
                    let source = unsafe { source.get_mut() };
                    let action = callback(fd, source, readiness.into());
                    Ok(match action {
                        FdAction::Continue => PostAction::Continue,
                        FdAction::Remove => PostAction::Remove,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Registers a descriptor without taking ownership. The caller must keep it open until the
    /// returned source is removed and all reactor dispatch has stopped.
    pub fn register_raw_fd_with_action<C>(
        &mut self,
        fd: RawFd,
        interest: Interest,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        C: FnMut(i32, FdReadiness) -> FdAction + 'static,
    {
        self.register_fd_with_action(BorrowedRawFd(fd), interest, callback)
    }

    /// Registers a one-shot or repeating timer.
    #[track_caller]
    pub fn register_timer<C>(
        &mut self,
        delay: Duration,
        repeat: bool,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        C: FnMut() + 'static,
    {
        let site = if timer_trace_enabled() {
            Some(Location::caller())
        } else {
            None
        };
        if let Some(site) = site {
            trace_log(format_args!(
                "register timer {}:{} delay={delay:?} repeat={repeat}",
                site.file(),
                site.line()
            ));
        }
        let mut callback = callback;
        self.event_loop
            .handle()
            .insert_source(Timer::from_duration(delay), move |_, _, _| {
                if let Some(site) = site {
                    trace_log(format_args!(
                        "fire timer {}:{} delay={delay:?} repeat={repeat}",
                        site.file(),
                        site.line()
                    ));
                }
                callback();
                if repeat {
                    TimeoutAction::ToDuration(delay)
                } else {
                    TimeoutAction::Drop
                }
            })
            .map_err(Into::into)
    }
}

/// Injectable wall-time source for the wall-clock timer.
///
/// Production code uses [`SystemWallClock`]; tests inject a fake cell-based
/// clock. Epoch milliseconds are timezone-independent, and minute/second
/// boundaries computed from them coincide with local minute/second
/// boundaries wherever the local UTC offset is minute-aligned.
pub trait WallClockSource {
    /// Current wall time as milliseconds since the Unix epoch.
    fn now_epoch_ms(&self) -> u64;
}

/// [`WallClockSource`] backed by `SystemTime`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClockSource for SystemWallClock {
    fn now_epoch_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }
}

/// Milliseconds until the next second (`with_seconds == true`) or minute
/// boundary after `now_epoch_ms`. Returns the full interval when already
/// exactly on a boundary so the timer always advances.
#[must_use]
pub fn wall_clock_delay_ms(now_epoch_ms: u64, with_seconds: bool) -> u64 {
    let interval_ms = if with_seconds { 1_000 } else { 60_000 };
    let remainder = now_epoch_ms % interval_ms;
    if remainder == 0 {
        interval_ms
    } else {
        interval_ms - remainder
    }
}

impl Reactor {
    /// Registers one self-rescheduling wall-clock timer.
    ///
    /// The initial timeout and every repeat are recomputed from the current
    /// wall time reported by `clock`, so drift never accumulates and a
    /// suspend/resume jump simply realigns to the next live boundary on the
    /// following tick (at most one catch-up tick fires after a forward jump).
    #[track_caller]
    pub fn register_wall_clock<C, W>(
        &mut self,
        clock: W,
        with_seconds: bool,
        callback: C,
    ) -> calloop::Result<RegistrationToken>
    where
        W: WallClockSource + 'static,
        C: FnMut() + 'static,
    {
        let site = if timer_trace_enabled() {
            Some(Location::caller())
        } else {
            None
        };
        let initial = wall_clock_delay_ms(clock.now_epoch_ms(), with_seconds);
        if let Some(site) = site {
            trace_log(format_args!(
                "register wall_clock {}:{} with_seconds={with_seconds} initial={initial}ms",
                site.file(),
                site.line()
            ));
        }
        let mut callback = callback;
        let clock = clock;
        self.event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_millis(initial.max(1))),
                move |_, _, _| {
                    if let Some(site) = site {
                        trace_log(format_args!(
                            "fire wall_clock {}:{} with_seconds={with_seconds}",
                            site.file(),
                            site.line()
                        ));
                    }
                    callback();
                    let next = wall_clock_delay_ms(clock.now_epoch_ms(), with_seconds);
                    TimeoutAction::ToDuration(Duration::from_millis(next.max(1)))
                },
            )
            .map_err(Into::into)
    }

    /// Removes a previously registered source.
    pub fn remove(&mut self, token: RegistrationToken) -> calloop::Result<()> {
        self.event_loop.handle().remove(token);
        Ok(())
    }

    /// Dispatches pending events, waiting no longer than `timeout` for the next event.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> calloop::Result<()> {
        let requested = timeout;
        let wake = self.wake.get();
        let timeout = bounded_timeout(timeout, wake, Instant::now());
        if timer_trace_enabled() {
            let source = if timeout != requested {
                "wake_deadline"
            } else {
                "caller"
            };
            trace_log(format_args!(
                "dispatch wait={timeout:?} source={source} requested={requested:?} wake_deadline={wake:?}"
            ));
        }
        self.event_loop.dispatch(timeout, &mut ())
    }
}

const TIMER_TRACE_ENV: &str = "FLAMEWM_TIMER_TRACE";

/// Parses the raw `FLAMEWM_TIMER_TRACE` value; only exactly `1` enables tracing.
#[must_use]
fn timer_trace_flag(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Reads the trace gate once per process; afterwards this is a single cached bool load.
#[inline]
fn timer_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| timer_trace_flag(std::env::var(TIMER_TRACE_ENV).ok().as_deref()))
}

fn trace_log(args: std::fmt::Arguments<'_>) {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    eprintln!(
        "[flamewm-timer +{:.3}ms] {args}",
        origin.elapsed().as_secs_f64() * 1000.0
    );
}

#[cfg(test)]
mod wake_tests {
    use super::*;

    fn assert_timeout(timeout: Option<u64>, at: Option<u64>, expected: Option<u64>) {
        let now = Instant::now();
        let ms = Duration::from_millis;
        let got = bounded_timeout(timeout.map(ms), at.map(|t| now + ms(t)), now);
        assert_eq!(got, expected.map(ms));
    }

    #[test]
    fn no_deadline_keeps_the_caller_timeout() {
        assert_timeout(None, None, None);
        assert_timeout(Some(3000), None, Some(3000));
    }

    #[test]
    fn deadline_bounds_an_unbounded_or_longer_timeout() {
        assert_timeout(None, Some(1200), Some(1200));
        assert_timeout(Some(5000), Some(1200), Some(1200));
        assert_timeout(Some(100), Some(1200), Some(100));
    }

    #[test]
    fn past_deadline_yields_zero_wait() {
        assert_timeout(None, Some(0), Some(0));
    }

    #[test]
    fn wake_deadline_handles_share_one_slot() {
        let reactor = Reactor::new().expect("reactor");
        let handle = reactor.wake_deadline();
        let clone = handle.clone();
        let at = Instant::now() + Duration::from_secs(1);
        clone.set(Some(at));
        assert_eq!(handle.get(), Some(at));
        clone.set(None);
        assert_eq!(handle.get(), None);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn fd_callback_can_remove_its_source() {
        let (reader, mut writer) = UnixStream::pair().expect("socket pair");
        let mut reactor = Reactor::new().expect("reactor");
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_for_callback = Arc::clone(&callback_count);

        reactor
            .register_fd_with_action(reader, Interest::READ, move |_, _| {
                callback_count_for_callback.fetch_add(1, Ordering::SeqCst);
                FdAction::Remove
            })
            .expect("register source");

        writer.write_all(&[1]).expect("write readiness byte");
        reactor
            .dispatch(Some(Duration::ZERO))
            .expect("dispatch first event");
        reactor
            .dispatch(Some(Duration::ZERO))
            .expect("dispatch after removal");

        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wall_clock_delay_aligns_to_second_and_minute_boundaries() {
        assert_eq!(wall_clock_delay_ms(61_234, false), 58_766);
        assert_eq!(wall_clock_delay_ms(1_250, true), 750);
        assert_eq!(wall_clock_delay_ms(60_000, false), 60_000);
        assert_eq!(wall_clock_delay_ms(1_000, true), 1_000);
        assert_eq!(wall_clock_delay_ms(0, false), 60_000);
    }

    struct FakeClock {
        now_ms: std::sync::Mutex<u64>,
    }

    impl FakeClock {
        fn new(now_ms: u64) -> Self {
            Self {
                now_ms: std::sync::Mutex::new(now_ms),
            }
        }

        fn advance(&self, delta_ms: u64) {
            *self.now_ms.lock().expect("fake clock") += delta_ms;
        }
    }

    impl WallClockSource for &'static FakeClock {
        fn now_epoch_ms(&self) -> u64 {
            *self.now_ms.lock().expect("fake clock")
        }
    }

    #[test]
    fn wall_clock_recomputes_delay_from_current_time_after_jump() {
        let fake: &'static FakeClock = Box::leak(Box::new(FakeClock::new(61_234)));
        assert_eq!(wall_clock_delay_ms(fake.now_epoch_ms(), false), 58_766);
        // Registration itself must not block or sleep; the delay math above
        // drives the first timeout.
        let mut reactor = Reactor::new().expect("reactor");
        let token = reactor
            .register_wall_clock(fake, false, || {})
            .expect("wall-clock source");
        reactor.remove(token).expect("remove wall-clock source");
        // Simulate a suspend/resume jump of one hour plus half a minute.
        fake.advance(3_630_000);
        assert_eq!(wall_clock_delay_ms(fake.now_epoch_ms(), false), 28_766);
        assert!(fake.now_epoch_ms() % 60_000 != 0);
    }
}

#[cfg(test)]
mod timer_trace_tests {
    use super::timer_trace_flag;

    #[test]
    fn timer_trace_gate_is_enabled_only_by_exact_one() {
        assert!(timer_trace_flag(Some("1")));
        assert!(!timer_trace_flag(None));
        assert!(!timer_trace_flag(Some("")));
        assert!(!timer_trace_flag(Some("0")));
        assert!(!timer_trace_flag(Some("true")));
    }
}
