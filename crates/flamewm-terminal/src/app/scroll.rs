//! Scrollback glide frames: one ticker thread that sleeps only while a frame is requested, and the per-frame step.
use crate::session::SessionEvent;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

use super::App;

/// Starts one thread that runs one glide frame per request, at least one `FRAME` after the request. Requests queue
/// on the returned sender; the thread exits when the sender is dropped.
pub(crate) fn spawn_frame_ticker(events: Sender<SessionEvent>, mut wake: UnixStream) -> Sender<()> {
    let (requests, pending) = mpsc::channel::<()>();
    // A glide ticker is an animation aid: when the OS refuses a thread the scrollback must keep working, so the
    // ticker degrades to instant scrolling (later sends fail and are ignored) instead of panicking.
    let spawned = std::thread::Builder::new()
        .name("flamewm-terminal-glide".into())
        .spawn(move || {
            while pending.recv().is_ok() {
                std::thread::sleep(crate::scroll::FRAME);
                if events.send(SessionEvent::Tick).is_err() {
                    break;
                }
                let _ = wake.write_all(&[1]);
            }
        });
    if spawned.is_err() {
        static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if WARNED.set(()).is_ok() {
            eprintln!("flamewm-terminal: glide ticker could not start; scrolling stays instant");
        }
    }
    requests
}

impl App {
    /// Requests one frame of the active glide. Returns whether a tick is pending; a ticker that could not start
    /// lets the caller snap instead of waiting for a frame that never comes.
    pub(crate) fn schedule_scroll_tick(&mut self) -> bool {
        if self.scroll_ticking {
            return true;
        }
        if self.scroll_timer.is_none() {
            let Ok(wake) = self.wake.try_clone() else {
                return false;
            };
            self.scroll_timer = Some(spawn_frame_ticker(self.events.clone(), wake));
        }
        let Some(timer) = self.scroll_timer.as_ref() else {
            return false;
        };
        if timer.send(()).is_ok() {
            self.scroll_ticking = true;
        } else {
            self.scroll_timer = None;
        }
        self.scroll_ticking
    }

    /// Applies one glide frame. An external writer that moved the pane's offset (Home, search, selection reset)
    /// cancels the glide instead of fighting it. Without a ticker the offset snaps to the target.
    pub(crate) fn advance_scroll(&mut self) {
        self.scroll_ticking = false;
        if !self.scroll.is_active() {
            return;
        }
        let now = Instant::now();
        let id = self.scroll.pane();
        let applied = self.scroll.applied();
        let target = self.scroll.target();
        let Some((offset, max)) = self
            .pane(id)
            .map(|pane| (pane.view_offset, pane.session.screen.scrollback_len()))
        else {
            self.scroll.cancel();
            return;
        };
        if offset != applied || target > max {
            self.scroll.cancel();
            return;
        }
        let next = self.scroll.advance(now);
        let finished = !self.scroll.is_animating(now);
        if let Some(pane) = self.pane_mut(id) {
            if let Some(value) = next {
                pane.view_offset = value;
            }
            if finished {
                pane.view_offset = target;
            }
        }
        if finished {
            return;
        }
        if !self.schedule_scroll_tick() {
            if let Some(pane) = self.pane_mut(id) {
                pane.view_offset = target;
            }
            self.scroll.cancel();
        }
    }
}
