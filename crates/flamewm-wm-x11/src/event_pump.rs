//! Pure coalescing pump for `Wm::run_with_hook`.
//!
//! Owner of the buffered-drain collapse: contiguous `MotionNotify` events for
//! the same `event` + `child` window collapse to the latest, and contiguous
//! `ConfigureRequest` events for the same `window` merge into one request that
//! keeps the latest value of every axis the client set. Any other event stops
//! the collapse, is stashed in `pending` exactly once, and a release is
//! therefore never crossed. No reordering.

use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConfigWindow, ConfigureRequestEvent, MotionNotifyEvent};

/// One pumped event plus coalescing metadata.
#[derive(Debug)]
pub struct PumpedEvent {
    /// Event to dispatch (latest motion or merged configure request when collapsed).
    pub event: Event,
    /// Total raw events observed for this emission (1 when untouched).
    pub received: usize,
    /// Events dropped by collapsing (`received - 1` for a collapsed run, else 0).
    pub coalesced: usize,
}

/// Pure coalescing pump. No connection access inside; callers supply events.
#[derive(Debug, Default)]
pub struct WmEventPump {
    pending: Option<Event>,
}

impl WmEventPump {
    #[must_use]
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// Next pumped event. `fetch_one` is the poll source (e.g. wrapping
    /// `conn.poll_for_event`); unit tests supply a scripted closure.
    /// Infallible convenience over [`WmEventPump::pump_next_result`].
    pub fn pump_next(
        &mut self,
        mut fetch_one: impl FnMut() -> Option<Event>,
    ) -> Option<PumpedEvent> {
        let mut fetch = || -> Result<Option<Event>, std::convert::Infallible> { Ok(fetch_one()) };
        self.pump_next_result(&mut fetch)
            .expect("infallible fetch cannot fail")
    }

    /// Error-aware fetch seam. `fetch_one` returns `Result` so transport
    /// errors (e.g. `ConnectionError`) propagate instead of being swallowed.
    /// Lookahead/pending ordering is identical to [`WmEventPump::pump_next`].
    pub fn pump_next_result<E>(
        &mut self,
        mut fetch_one: impl FnMut() -> Result<Option<Event>, E>,
    ) -> Result<Option<PumpedEvent>, E> {
        let first = match self.pending.take() {
            Some(event) => event,
            None => match fetch_one()? {
                Some(event) => event,
                None => return Ok(None),
            },
        };
        let mut pending = None;
        let pumped = collapse_motions_result(first, &mut fetch_one, &mut pending)?;
        debug_assert!(self.pending.is_none());
        self.pending = pending;
        Ok(Some(pumped))
    }

    /// Live-X convenience: `pending.take()` first, else `poll_for_event`.
    /// Collapse/drain uses repeated `poll_for_event`; lookahead stashed once.
    /// All `ConnectionError`s propagate; nothing is swallowed.
    pub fn next<C: x11rb::connection::Connection>(
        &mut self,
        conn: &C,
    ) -> Result<Option<PumpedEvent>, x11rb::errors::ConnectionError> {
        self.pump_next_result(|| conn.poll_for_event())
    }

    #[cfg(test)]
    fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Error-aware collapse of one buffered run. Motion runs keep the latest motion;
/// configure runs merge same-window requests; every other event is emitted
/// untouched. Drain errors abort the run and propagate: an absorbed prefix is
/// kept in the returned `PumpedEvent` and no lookahead is stashed on the error
/// path, so a release is never crossed.
pub fn collapse_motions_result<E>(
    first: Event,
    drain: &mut dyn FnMut() -> Result<Option<Event>, E>,
    pending_slot: &mut Option<Event>,
) -> Result<PumpedEvent, E> {
    match first {
        Event::MotionNotify(seed) => collapse_motion_run_result(seed, drain, pending_slot),
        Event::ConfigureRequest(seed) => collapse_configure_run_result(seed, drain, pending_slot),
        other => Ok(PumpedEvent {
            event: other,
            received: 1,
            coalesced: 0,
        }),
    }
}

/// Motion-only run: latest same-target motion wins, lookahead stashed once.
fn collapse_motion_run_result<E>(
    seed: MotionNotifyEvent,
    drain: &mut dyn FnMut() -> Result<Option<Event>, E>,
    pending_slot: &mut Option<Event>,
) -> Result<PumpedEvent, E> {
    debug_assert!(pending_slot.is_none());
    let (target_event, target_child) = (seed.event, seed.child);
    let mut latest = Event::MotionNotify(seed);
    let mut received: usize = 1;
    while let Some(next) = drain()? {
        match next {
            Event::MotionNotify(motion)
                if motion.event == target_event && motion.child == target_child =>
            {
                latest = Event::MotionNotify(motion);
                received += 1;
            }
            other => {
                *pending_slot = Some(other);
                break;
            }
        }
    }
    Ok(PumpedEvent {
        event: latest,
        received,
        coalesced: received.saturating_sub(1),
    })
}

/// Configure-only run: same-window requests merge, lookahead stashed once.
fn collapse_configure_run_result<E>(
    seed: ConfigureRequestEvent,
    drain: &mut dyn FnMut() -> Result<Option<Event>, E>,
    pending_slot: &mut Option<Event>,
) -> Result<PumpedEvent, E> {
    debug_assert!(pending_slot.is_none());
    let window = seed.window;
    let mut latest = seed;
    let mut received: usize = 1;
    while let Some(next) = drain()? {
        match next {
            Event::ConfigureRequest(request) if request.window == window => {
                latest = merge_configure_request(latest, request);
                received += 1;
            }
            other => {
                *pending_slot = Some(other);
                break;
            }
        }
    }
    Ok(PumpedEvent {
        event: Event::ConfigureRequest(latest),
        received,
        coalesced: received.saturating_sub(1),
    })
}

/// Merge two same-window `ConfigureRequest`s. The later request wins for every
/// axis it sets; axes only the earlier request set keep that value and stay in
/// the unioned mask. Applying the merge is equivalent to applying both requests
/// in order, so the client's final geometry and pixels are unchanged.
fn merge_configure_request(
    seed: ConfigureRequestEvent,
    next: ConfigureRequestEvent,
) -> ConfigureRequestEvent {
    debug_assert_eq!(seed.window, next.window);
    let mut merged = next;
    merged.value_mask = seed.value_mask | next.value_mask;
    if !next.value_mask.contains(ConfigWindow::X) {
        merged.x = seed.x;
    }
    if !next.value_mask.contains(ConfigWindow::Y) {
        merged.y = seed.y;
    }
    if !next.value_mask.contains(ConfigWindow::WIDTH) {
        merged.width = seed.width;
    }
    if !next.value_mask.contains(ConfigWindow::HEIGHT) {
        merged.height = seed.height;
    }
    if !next.value_mask.contains(ConfigWindow::BORDER_WIDTH) {
        merged.border_width = seed.border_width;
    }
    if !next.value_mask.contains(ConfigWindow::SIBLING) {
        merged.sibling = seed.sibling;
    }
    if !next.value_mask.contains(ConfigWindow::STACK_MODE) {
        merged.stack_mode = seed.stack_mode;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{
        ButtonPressEvent, ButtonReleaseEvent, ConfigWindow, ConfigureNotifyEvent,
        ConfigureRequestEvent, KeyButMask, Motion, MotionNotifyEvent, StackMode,
    };

    fn motion(event: u32, child: u32, root_x: i16) -> Event {
        Event::MotionNotify(MotionNotifyEvent {
            response_type: 0,
            detail: Motion::NORMAL,
            sequence: 0,
            time: 0,
            root: 1,
            event,
            child,
            root_x,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::default(),
            same_screen: true,
        })
    }

    fn press() -> Event {
        Event::ButtonPress(ButtonPressEvent {
            response_type: 0,
            detail: 1,
            sequence: 0,
            time: 0,
            root: 1,
            event: 7,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::default(),
            same_screen: true,
        })
    }

    fn release() -> Event {
        Event::ButtonRelease(ButtonReleaseEvent {
            response_type: 0,
            detail: 1,
            sequence: 0,
            time: 0,
            root: 1,
            event: 7,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::default(),
            same_screen: true,
        })
    }

    fn configure() -> Event {
        Event::ConfigureNotify(ConfigureNotifyEvent {
            response_type: 0,
            sequence: 0,
            event: 7,
            window: 7,
            above_sibling: 0,
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            border_width: 0,
            override_redirect: false,
        })
    }

    fn configure_request(
        window: u32,
        mask: ConfigWindow,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    ) -> Event {
        Event::ConfigureRequest(ConfigureRequestEvent {
            response_type: 23,
            stack_mode: StackMode::ABOVE,
            sequence: 0,
            parent: 1,
            window,
            sibling: 0,
            x,
            y,
            width,
            height,
            border_width: 0,
            value_mask: mask,
        })
    }

    fn script(events: Vec<Event>) -> impl FnMut() -> Option<Event> {
        let mut iter = events.into_iter();
        move || iter.next()
    }

    fn root_x_of(event: &Event) -> i16 {
        match event {
            Event::MotionNotify(motion) => motion.root_x,
            _ => panic!("expected motion"),
        }
    }

    #[test]
    fn motion_x3_collapses_to_latest() {
        let mut pump = WmEventPump::new();
        let out = pump
            .pump_next(script(vec![
                motion(7, 0, 1),
                motion(7, 0, 2),
                motion(7, 0, 3),
            ]))
            .expect("event");
        assert_eq!(root_x_of(&out.event), 3);
        assert_eq!(out.received, 3);
        assert_eq!(out.coalesced, 2);
        assert!(!pump.has_pending());
    }

    #[test]
    fn motion_release_motion_stops_and_release_next() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![motion(7, 0, 1), release()]))
            .expect("event");
        assert_eq!(root_x_of(&first.event), 1);
        assert_eq!((first.received, first.coalesced), (1, 0));
        // Pending returned exactly once, then drain continues.
        let second = pump.pump_next(script(vec![])).expect("event");
        assert!(matches!(second.event, Event::ButtonRelease(_)));
        assert!(!pump.has_pending());
        let third = pump
            .pump_next(script(vec![motion(7, 0, 9)]))
            .expect("event");
        assert_eq!(root_x_of(&third.event), 9);
    }

    #[test]
    fn different_event_window_does_not_collapse() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![motion(1, 0, 1), motion(2, 0, 2)]))
            .expect("event");
        assert_eq!(root_x_of(&first.event), 1);
        assert_eq!((first.received, first.coalesced), (1, 0));
        let second = pump.pump_next(script(vec![])).expect("event");
        assert_eq!(root_x_of(&second.event), 2);
        assert!(!pump.has_pending());
    }

    #[test]
    fn same_window_different_child_does_not_collapse() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![motion(7, 10, 1), motion(7, 11, 2)]))
            .expect("event");
        assert_eq!(root_x_of(&first.event), 1);
        assert_eq!((first.received, first.coalesced), (1, 0));
        let second = pump.pump_next(script(vec![])).expect("event");
        assert_eq!(root_x_of(&second.event), 2);
        assert!(!pump.has_pending());
    }

    #[test]
    fn non_motion_untouched_and_press_stops_collapse() {
        let mut pump = WmEventPump::new();
        let out = pump.pump_next(script(vec![configure()])).expect("event");
        assert!(matches!(out.event, Event::ConfigureNotify(_)));
        assert_eq!((out.received, out.coalesced), (1, 0));

        let first = pump
            .pump_next(script(vec![motion(7, 0, 5), press()]))
            .expect("event");
        assert_eq!(root_x_of(&first.event), 5);
        assert_eq!((first.received, first.coalesced), (1, 0));
        let second = pump.pump_next(script(vec![])).expect("event");
        assert!(matches!(second.event, Event::ButtonPress(_)));
        assert!(!pump.has_pending());
    }

    #[test]
    fn pending_returned_exactly_once() {
        let mut pump = WmEventPump::new();
        let _ = pump.pump_next(script(vec![motion(7, 0, 1), release()]));
        assert!(pump.has_pending());
        let _ = pump.pump_next(script(vec![])).expect("pending");
        assert!(!pump.has_pending());
        assert!(pump.pump_next(script(vec![])).is_none());
    }

    #[derive(Debug, PartialEq)]
    struct FetchErr;

    #[test]
    fn result_seam_propagates_fetch_error_on_first() {
        let mut pump = WmEventPump::new();
        let mut failing = || -> Result<Option<Event>, FetchErr> { Err(FetchErr) };
        assert!(matches!(pump.pump_next_result(&mut failing), Err(FetchErr)));
        assert!(!pump.has_pending());
    }

    #[test]
    fn result_seam_propagates_drain_error_without_crossing_release() {
        let mut pump = WmEventPump::new();
        let mut calls = 0;
        let mut fetch = || -> Result<Option<Event>, FetchErr> {
            calls += 1;
            match calls {
                1 => Ok(Some(motion(7, 0, 1))),
                2 => Ok(Some(motion(7, 0, 2))),
                _ => Err(FetchErr),
            }
        };
        assert!(matches!(pump.pump_next_result(&mut fetch), Err(FetchErr)));
        // Nothing stashed: the release (never yet seen) was not crossed.
        assert!(!pump.has_pending());
    }

    #[test]
    fn result_seam_ok_path_preserves_lookahead_and_release() {
        let mut pump = WmEventPump::new();
        let events = vec![motion(7, 0, 1), motion(7, 0, 2), release()];
        let mut iter = events.into_iter();
        let mut fetch = || -> Result<Option<Event>, FetchErr> { Ok(iter.next()) };
        let first = pump
            .pump_next_result(&mut fetch)
            .expect("event")
            .expect("some");
        assert_eq!(root_x_of(&first.event), 2);
        assert_eq!((first.received, first.coalesced), (2, 1));
        assert!(pump.has_pending());
        let second = pump
            .pump_next_result(&mut fetch)
            .expect("event")
            .expect("some");
        assert!(matches!(second.event, Event::ButtonRelease(_)));
        assert!(!pump.has_pending());
    }

    #[test]
    fn configure_x3_merges_axes_and_masks() {
        let mut pump = WmEventPump::new();
        let out = pump
            .pump_next(script(vec![
                configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
                configure_request(7, ConfigWindow::WIDTH, 0, 0, 20, 0),
                configure_request(7, ConfigWindow::Y | ConfigWindow::HEIGHT, 0, 3, 0, 30),
            ]))
            .expect("event");
        assert_eq!((out.received, out.coalesced), (3, 2));
        let Event::ConfigureRequest(req) = out.event else {
            panic!("expected configure request");
        };
        assert_eq!(req.window, 7);
        assert_eq!(
            req.value_mask,
            ConfigWindow::X | ConfigWindow::Y | ConfigWindow::WIDTH | ConfigWindow::HEIGHT
        );
        assert_eq!((req.x, req.y), (1, 3));
        assert_eq!((req.width, req.height), (20, 30));
        assert!(!pump.has_pending());
    }

    #[test]
    fn configure_same_axis_latest_wins() {
        let mut pump = WmEventPump::new();
        let out = pump
            .pump_next(script(vec![
                configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
                configure_request(7, ConfigWindow::X, 9, 0, 0, 0),
            ]))
            .expect("event");
        assert_eq!((out.received, out.coalesced), (2, 1));
        let Event::ConfigureRequest(req) = out.event else {
            panic!("expected configure request");
        };
        assert_eq!(req.x, 9);
    }

    #[test]
    fn configure_different_window_does_not_collapse() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![
                configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
                configure_request(8, ConfigWindow::X, 2, 0, 0, 0),
            ]))
            .expect("event");
        assert_eq!((first.received, first.coalesced), (1, 0));
        let second = pump.pump_next(script(vec![])).expect("event");
        let Event::ConfigureRequest(req) = second.event else {
            panic!("expected configure request");
        };
        assert_eq!(req.window, 8);
        assert!(!pump.has_pending());
    }

    #[test]
    fn configure_run_stops_at_motion_and_pending_is_returned_once() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![
                configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
                motion(7, 0, 5),
            ]))
            .expect("event");
        assert!(matches!(first.event, Event::ConfigureRequest(_)));
        let second = pump.pump_next(script(vec![])).expect("event");
        assert_eq!(root_x_of(&second.event), 5);
        assert!(!pump.has_pending());
        assert!(pump.pump_next(script(vec![])).is_none());
    }

    #[test]
    fn configure_run_stops_at_release_without_crossing_it() {
        let mut pump = WmEventPump::new();
        let first = pump
            .pump_next(script(vec![
                configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
                release(),
            ]))
            .expect("event");
        assert_eq!((first.received, first.coalesced), (1, 0));
        let second = pump.pump_next(script(vec![])).expect("event");
        assert!(matches!(second.event, Event::ButtonRelease(_)));
        assert!(!pump.has_pending());
    }

    #[test]
    fn result_seam_configure_run_propagates_drain_error() {
        let mut pump = WmEventPump::new();
        let mut calls = 0;
        let mut fetch = || -> Result<Option<Event>, FetchErr> {
            calls += 1;
            match calls {
                1 => Ok(Some(configure_request(7, ConfigWindow::X, 1, 0, 0, 0))),
                _ => Err(FetchErr),
            }
        };
        assert!(matches!(pump.pump_next_result(&mut fetch), Err(FetchErr)));
        assert!(!pump.has_pending());
    }

    #[test]
    fn result_seam_configure_run_preserves_lookahead_and_release() {
        let mut pump = WmEventPump::new();
        let events = vec![
            configure_request(7, ConfigWindow::X, 1, 0, 0, 0),
            configure_request(7, ConfigWindow::WIDTH, 0, 0, 20, 0),
            release(),
        ];
        let mut iter = events.into_iter();
        let mut fetch = || -> Result<Option<Event>, FetchErr> { Ok(iter.next()) };
        let first = pump
            .pump_next_result(&mut fetch)
            .expect("event")
            .expect("some");
        assert_eq!((first.received, first.coalesced), (2, 1));
        let Event::ConfigureRequest(req) = first.event else {
            panic!("expected configure request");
        };
        assert_eq!(req.value_mask, ConfigWindow::X | ConfigWindow::WIDTH);
        assert_eq!((req.x, req.width), (1, 20));
        assert!(pump.has_pending());
        let second = pump
            .pump_next_result(&mut fetch)
            .expect("event")
            .expect("some");
        assert!(matches!(second.event, Event::ButtonRelease(_)));
        assert!(!pump.has_pending());
    }
}
