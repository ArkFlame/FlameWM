//! Alt+Tab window switcher: most-recently-used order, cycling, commit and cancel.
//!
//! The WM owns the selection and the focus change. The shell only renders the snapshot this
//! module publishes as `WindowSwitcherChanged`; if nothing renders it, the switch still works.
//! Keys arrive from the `XInput2` raw feed (see `raw_keys`), so the switcher works while a
//! fullscreen client holds the keyboard. The pure parts (`Mru`, `switcher_order`, `Switcher`)
//! carry no X state and are unit tested; the `Wm` glue at the bottom performs the X calls.

use super::*;
use flamewm_control_wire::{ControlSignal, SwitcherEntry};

/// Most-recently-focused windows remembered for the switcher order.
const MRU_LIMIT: usize = 64;

const XK_TAB: u32 = 0xff09;
const XK_LEFT_TAB: u32 = 0xfe20;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_KP_ENTER: u32 = 0xff8d;

/// Direction of one selection step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Next,
    Prev,
}

/// Focus history of managed clients, most recent first.
#[derive(Debug, Default)]
pub(crate) struct Mru(Vec<Window>);

impl Mru {
    pub(crate) fn touch(&mut self, window: Window) {
        self.0.retain(|&known| known != window);
        self.0.insert(0, window);
        self.0.truncate(MRU_LIMIT);
    }

    pub(crate) fn order(&self) -> &[Window] {
        &self.0
    }

    pub(crate) fn prune(&mut self, alive: impl Fn(Window) -> bool) {
        self.0.retain(|&window| alive(window));
    }
}

/// Switcher order: focus history first, then never-focused windows in stacking order (top
/// first). Only `eligible` windows appear, and each appears once.
pub(crate) fn switcher_order(
    mru: &[Window],
    stack_top_first: &[Window],
    eligible: impl Fn(Window) -> bool,
) -> Vec<Window> {
    let mut out: Vec<Window> = mru.iter().copied().filter(|&w| eligible(w)).collect();
    for &window in stack_top_first {
        if eligible(window) && !out.contains(&window) {
            out.push(window);
        }
    }
    out
}

#[derive(Debug, Default)]
pub(crate) struct Switcher {
    session: Option<Session>,
}

#[derive(Debug)]
struct Session {
    windows: Vec<Window>,
    selected: usize,
    /// Selection presses in this session, the opening one included.
    presses: u32,
}

impl Switcher {
    pub(crate) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Opens on `windows` (MRU order). A forward open selects the previous window, the one
    /// directly behind the active window; a backward open selects the last. Returns false and
    /// stays closed when there is nothing to show.
    pub(crate) fn open(&mut self, windows: Vec<Window>, step: Step) -> bool {
        let len = windows.len();
        if len == 0 {
            return false;
        }
        let selected = match step {
            Step::Next => 1.min(len - 1),
            Step::Prev => len - 1,
        };
        self.session = Some(Session {
            windows,
            selected,
            presses: 1,
        });
        true
    }

    /// One more selection press. Wraps around the list.
    pub(crate) fn advance(&mut self, step: Step) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let len = session.windows.len();
        if len == 0 {
            return;
        }
        session.selected = match step {
            Step::Next => (session.selected + 1) % len,
            Step::Prev => (session.selected + len - 1) % len,
        };
        session.presses = session.presses.saturating_add(1);
    }

    /// True once a second selection press arrived: the overlay shows without the hold delay.
    pub(crate) fn immediate(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.presses >= 2)
    }

    /// Ends the session and returns the selected window, if any.
    pub(crate) fn commit(&mut self) -> Option<Window> {
        self.session
            .take()
            .and_then(|session| session.windows.get(session.selected).copied())
    }

    /// Ends the session without a target. Focus is left alone.
    pub(crate) fn cancel(&mut self) {
        self.session = None;
    }

    /// Open session's list and selection index, for the overlay snapshot.
    pub(crate) fn view(&self) -> Option<(&[Window], usize)> {
        self.session
            .as_ref()
            .map(|session| (session.windows.as_slice(), session.selected))
    }
}

/// Maps a keysym to the switcher action it performs while the switcher is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitcherKey {
    Step(Step),
    Commit,
    Cancel,
    Ignore,
}

/// Tab and the arrows step; Return commits; Escape cancels. `shift` only changes Tab's
/// direction; arrows use their own direction.
pub(crate) fn classify_switcher_key(keysym: u32, shift: bool) -> SwitcherKey {
    match keysym {
        XK_TAB | XK_LEFT_TAB => SwitcherKey::Step(if shift { Step::Prev } else { Step::Next }),
        XK_RIGHT | XK_DOWN => SwitcherKey::Step(Step::Next),
        XK_LEFT | XK_UP => SwitcherKey::Step(Step::Prev),
        XK_RETURN | XK_KP_ENTER => SwitcherKey::Commit,
        XK_ESCAPE => SwitcherKey::Cancel,
        _ => SwitcherKey::Ignore,
    }
}

fn closed_signal() -> ControlSignal {
    ControlSignal::WindowSwitcherChanged {
        open: false,
        immediate: false,
        selected: 0,
        entries: Vec::new(),
    }
}

impl<C: Connection> Wm<'_, C> {
    /// Alt+Tab press. Opens the switcher on the first press, steps it on later ones.
    pub(super) fn switcher_step(&mut self, step: Step) -> Result<(), ReplyError> {
        if self.switcher.is_open() {
            self.switcher.advance(step);
        } else {
            let windows = self.switcher_candidates()?;
            if !self.switcher.open(windows, step) {
                return Ok(());
            }
            // No keyboard grab: a WM-held grab stops the raw feed from delivering the Alt
            // release, so the switcher would never commit. Keys reach the WM via the raw feed.
        }
        self.publish_switcher();
        Ok(())
    }

    /// Managed normal windows on this workspace (iconic ones included) in switcher order.
    fn switcher_candidates(&mut self) -> Result<Vec<Window>, ReplyError> {
        let clients = &self.clients;
        self.mru.prune(|window| clients.contains_key(&window));
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        let stack: Vec<Window> = tree
            .children
            .iter()
            .rev()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .collect();
        let current = self.current_workspace;
        Ok(switcher_order(self.mru.order(), &stack, |client| {
            self.clients.get(&client).is_some_and(|state| {
                state.kind == WindowKind::Normal && (state.sticky || state.workspace == current)
            })
        }))
    }

    /// Records the overlay snapshot for the next reactor flush.
    fn publish_switcher(&mut self) {
        let immediate = self.switcher.immediate();
        self.switcher_signal = Some(match self.switcher.view() {
            Some((windows, selected)) => ControlSignal::WindowSwitcherChanged {
                open: true,
                immediate,
                selected: u32::try_from(selected).unwrap_or(0),
                entries: windows
                    .iter()
                    .map(|window| self.switcher_entry(*window))
                    .collect(),
            },
            None => closed_signal(),
        });
    }

    fn switcher_entry(&self, window: Window) -> SwitcherEntry {
        match self.clients.get(&window) {
            Some(state) => SwitcherEntry {
                title: state.title.to_string(),
                app_id: state.wm_class.to_string(),
            },
            None => SwitcherEntry {
                title: String::new(),
                app_id: String::new(),
            },
        }
    }

    /// Dispatches one key press while the switcher is open. Shared by both key feeds.
    pub(super) fn switcher_key(&mut self, keycode: u8, state: u16) -> Result<(), ReplyError> {
        let mapping = self.conn.get_keyboard_mapping(keycode, 1)?.reply()?;
        let keysym = mapping.keysyms.first().copied().unwrap_or(0);
        match classify_switcher_key(
            keysym,
            state & flamewm_x11_desktop::shortcuts::MOD_SHIFT != 0,
        ) {
            SwitcherKey::Step(step) => self.switcher_step(step),
            SwitcherKey::Commit => self.switcher_commit(),
            SwitcherKey::Cancel => self.switcher_cancel(),
            SwitcherKey::Ignore => Ok(()),
        }
    }

    /// Alt released (or Return): focuses the selected window, un-iconifying it first. A
    /// fullscreen client that keeps the keyboard is taken out of fullscreen, so the target
    /// can receive keys.
    pub(super) fn switcher_commit(&mut self) -> Result<(), ReplyError> {
        let target = self.switcher.commit();
        self.end_switcher_session()?;
        let Some(target) = target.filter(|target| self.clients.contains_key(target)) else {
            return Ok(());
        };
        let others: Vec<Window> = self
            .fullscreen_clients()
            .into_iter()
            .filter(|&client| client != target)
            .collect();
        if !others.is_empty() && self.keyboard_grabbed_by_other()? {
            for client in others {
                self.set_fullscreen(client, false)?;
                self.minimize(client)?;
            }
        }
        if self
            .clients
            .get(&target)
            .is_some_and(|state| state.minimized)
        {
            self.restore_minimized(target)?;
        }
        self.focus(target)
    }

    /// Escape: closes the overlay and leaves focus where it was.
    pub(super) fn switcher_cancel(&mut self) -> Result<(), ReplyError> {
        self.switcher.cancel();
        self.end_switcher_session()
    }

    fn end_switcher_session(&mut self) -> Result<(), ReplyError> {
        self.switcher_signal = Some(closed_signal());
        Ok(())
    }

    /// Takes the overlay snapshot queued by the last switcher change, if any.
    pub(crate) fn take_switcher_signal(&mut self) -> Option<ControlSignal> {
        self.switcher_signal.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mru_touch_moves_to_front_without_duplicates() {
        let mut mru = Mru::default();
        mru.touch(1);
        mru.touch(2);
        mru.touch(3);
        mru.touch(1);
        assert_eq!(mru.order(), &[1, 3, 2]);
    }

    #[test]
    fn mru_prune_drops_dead_windows() {
        let mut mru = Mru::default();
        mru.touch(5);
        mru.touch(6);
        mru.prune(|window| window != 5);
        assert_eq!(mru.order(), &[6]);
    }

    #[test]
    fn order_is_mru_then_unfocused_stack_and_skips_ineligible() {
        // Stack top first: 9 is never focused and sits on top.
        let order = switcher_order(&[2, 4, 7], &[9, 4, 3, 2], |w| w != 7);
        assert_eq!(order, vec![2, 4, 9, 3]);
    }

    #[test]
    fn forward_open_selects_the_previous_window() {
        let mut switcher = Switcher::default();
        assert!(switcher.open(vec![10, 20, 30], Step::Next));
        assert_eq!(switcher.view(), Some((&[10, 20, 30][..], 1)));
    }

    #[test]
    fn backward_open_selects_the_last_window() {
        let mut switcher = Switcher::default();
        assert!(switcher.open(vec![10, 20, 30], Step::Prev));
        assert_eq!(switcher.view(), Some((&[10, 20, 30][..], 2)));
    }

    #[test]
    fn single_window_and_empty_list_select_index_zero_or_stay_closed() {
        let mut switcher = Switcher::default();
        assert!(!switcher.open(Vec::new(), Step::Next));
        assert!(!switcher.is_open());
        assert!(switcher.open(vec![10], Step::Next));
        assert_eq!(switcher.view(), Some((&[10][..], 0)));
    }

    #[test]
    fn cycling_wraps_both_ways() {
        let mut switcher = Switcher::default();
        switcher.open(vec![1, 2, 3], Step::Next);
        switcher.advance(Step::Next);
        assert_eq!(switcher.view().map(|(_, s)| s), Some(2));
        switcher.advance(Step::Next);
        assert_eq!(switcher.view().map(|(_, s)| s), Some(0));
        switcher.advance(Step::Prev);
        assert_eq!(switcher.view().map(|(_, s)| s), Some(2));
    }

    #[test]
    fn quick_tap_commits_the_previous_window_without_immediate_overlay() {
        let mut switcher = Switcher::default();
        switcher.open(vec![1, 2, 3], Step::Next);
        assert!(!switcher.immediate(), "one press waits for the hold delay");
        assert_eq!(switcher.commit(), Some(2));
        assert!(!switcher.is_open());
    }

    #[test]
    fn second_press_makes_the_overlay_immediate() {
        let mut switcher = Switcher::default();
        switcher.open(vec![1, 2, 3], Step::Next);
        switcher.advance(Step::Next);
        assert!(switcher.immediate());
    }

    #[test]
    fn commit_returns_the_selected_window_and_closes() {
        let mut switcher = Switcher::default();
        switcher.open(vec![1, 2, 3], Step::Next);
        switcher.advance(Step::Next);
        assert_eq!(switcher.commit(), Some(3));
        assert_eq!(switcher.commit(), None, "a closed session commits nothing");
    }

    #[test]
    fn cancel_closes_without_a_target() {
        let mut switcher = Switcher::default();
        switcher.open(vec![1, 2], Step::Next);
        switcher.cancel();
        assert!(!switcher.is_open());
        assert_eq!(switcher.commit(), None);
    }

    #[test]
    fn keysyms_map_to_steps_commit_and_cancel() {
        assert_eq!(
            classify_switcher_key(XK_TAB, false),
            SwitcherKey::Step(Step::Next)
        );
        assert_eq!(
            classify_switcher_key(XK_TAB, true),
            SwitcherKey::Step(Step::Prev)
        );
        assert_eq!(
            classify_switcher_key(XK_LEFT_TAB, false),
            SwitcherKey::Step(Step::Next)
        );
        assert_eq!(
            classify_switcher_key(XK_LEFT, false),
            SwitcherKey::Step(Step::Prev)
        );
        assert_eq!(
            classify_switcher_key(XK_DOWN, false),
            SwitcherKey::Step(Step::Next)
        );
        assert_eq!(classify_switcher_key(XK_RETURN, false), SwitcherKey::Commit);
        assert_eq!(classify_switcher_key(XK_ESCAPE, false), SwitcherKey::Cancel);
        assert_eq!(classify_switcher_key(0x61, false), SwitcherKey::Ignore);
    }
}
