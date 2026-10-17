//! Root-press observer for hosts whose menus close on an outside press. The
//! press is observed only: never grabbed, replayed or consumed.

use super::*;
use crate::runtime::{RawPress, RawPressWatch};
use crate::surface_controller::{grab_popup_presses, release_popup_presses, PRESS_OBSERVER};
use crate::xinput::*;

impl X11App {
    /// Arms or disarms the root press observer to match the host's request.
    /// Idempotent; disarming drops presses that were not yet delivered.
    pub(crate) unsafe fn sync_raw_press_observer(&mut self, watch: &RawPressWatch) {
        let want = watch.get();
        if want == self.press_observed {
            return;
        }
        self.press_observed = want;
        // SAFETY: live display owned by this app; root is a plain query.
        let root = unsafe { XRootWindow(self.display, self.screen) };
        if want {
            // SAFETY: same live display and root; the selection outlives the request.
            unsafe { grab_popup_presses(self.display, root, &mut self.press_selection, PRESS_OBSERVER) };
        } else {
            self.raw_presses.clear();
            // SAFETY: same live display; armed by the branch above on an earlier turn.
            unsafe { release_popup_presses(self.display, root, &mut self.press_selection, PRESS_OBSERVER) };
        }
    }

    /// Records one root RawButtonPress while armed. `event` is a GenericEvent
    /// already checked against the selection's XI opcode by the caller.
    pub(crate) unsafe fn record_raw_press(&mut self, event: &XEvent, scale: f32) {
        if !self.press_observed {
            return;
        }
        // SAFETY: `event` is a GenericEvent per the caller's check.
        let mut cookie = unsafe { generic_cookie(event) };
        // SAFETY: the cookie is a copy of this XNextEvent generic event; Xlib fills `data`.
        if unsafe { XGetEventData(self.display, &mut cookie) } == 0 {
            return;
        }
        // SAFETY: XGetEventData succeeded, so `data` is an XIRawEvent until XFreeEventData.
        let raw = unsafe { *(cookie.data as *const XIRawEvent) };
        // SAFETY: pairs with the successful XGetEventData above.
        unsafe { XFreeEventData(self.display, &mut cookie) };
        if raw.evtype != XI_RAW_BUTTON_PRESS {
            return;
        }
        // SAFETY: live display and root; out-pointers are locals.
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let (mut root_return, mut child_return): (Window, Window) = (0, 0);
        let (mut x, mut y, mut win_x, mut win_y) = (0, 0, 0, 0);
        let mut mask: std::os::raw::c_uint = 0;
        // SAFETY: live connection; out-pointers are locals.
        let on_screen = unsafe {
            XQueryPointer(
                self.display,
                root,
                &mut root_return,
                &mut child_return,
                &mut x,
                &mut y,
                &mut win_x,
                &mut win_y,
                &mut mask,
            )
        };
        if on_screen == 0 {
            return;
        }
        self.raw_presses.push(RawPress {
            x: x as f32 / scale,
            y: y as f32 / scale,
            button: raw.detail as u32,
            time_ms: raw.time as u64,
        });
    }

    /// Hands the presses of the last X turn to the host, then presents what
    /// they changed: the next wake may block on the reactor.
    pub(crate) unsafe fn deliver_raw_presses<P>(
        &mut self,
        document: &mut RuntimeDocument,
        on_press: &mut P,
    ) -> Result<(), String>
    where
        P: FnMut(&RawPress, &mut RuntimeDocument) -> Result<(), String>,
    {
        let presses = std::mem::take(&mut self.raw_presses);
        for press in &presses {
            on_press(press, document)?;
        }
        if !presses.is_empty() {
            unsafe { self.redraw_if_dirty(document)? };
        }
        Ok(())
    }
}
