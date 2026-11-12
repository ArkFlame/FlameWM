//! Alt+Tab window switcher: most-recently-used order, cycling, commit and cancel.
//!
//! The WM owns the selection and the focus change. The shell only renders the snapshot this
//! module publishes as `WindowSwitcherChanged`; if nothing renders it, the switch still works.
//! Keys arrive from the `XInput2` raw feed (see `raw_keys`), so the switcher works while a
//! fullscreen client holds the keyboard. The pure parts (`Mru`, `switcher_order`, `Switcher`)
//! carry no X state and are unit tested; the `Wm` glue at the bottom performs the X calls.

use super::switcher_thumb_provider::CaptureBudget;
use super::switcher_thumb_x11::X11ThumbSource;
use super::switcher_thumbs::{MAX_THUMBS, THUMB_MAX_H, THUMB_MAX_W, ThumbKey};
use super::*;
use flamewm_api::switcher::SwitcherThumb;
use flamewm_control_wire::{ControlSignal, SwitcherEntry};
use std::hash::{Hash, Hasher};

/// Most-recently-focused windows remembered for the switcher order.
const MRU_LIMIT: usize = 64;

/// Captures the event loop may run per turn while the switcher is open. One keeps a full-size
/// window (about 10 ms to read and downscale at 1920x1080) under the 16 ms turn budget.
pub(crate) const SWITCHER_CAPTURES_PER_TURN: u32 = 1;

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

/// Converts a downscaled X `ZPixmap` buffer (32-bit BGRX, rows packed) to straight RGBA8 with
/// opaque alpha. `None` when the buffer length does not match the declared size.
pub(crate) fn thumb_rgba(width: u32, height: u32, bgrx: &[u8]) -> Option<Vec<u8>> {
    if width > THUMB_MAX_W || height > THUMB_MAX_H {
        return None;
    }
    let len = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if bgrx.len() != len {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    for px in bgrx.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    Some(out)
}

fn closed_signal() -> ControlSignal {
    ControlSignal::WindowSwitcherChanged {
        open: false,
        immediate: false,
        selected: 0,
        entries: Vec::new(),
    }
}

/// Which previews the shell already holds for this switcher session. A window keeps its
/// generation while its pixels are unchanged; the shell resolves the rest from its cache.
#[derive(Debug, Default)]
pub(crate) struct PreviewLedger {
    /// Window -> (content hash, generation) of the pixels last sent for it.
    sent: HashMap<Window, (u64, u64)>,
    last: u64,
}

impl PreviewLedger {
    /// Generation already sent for `window` with this content hash, if any.
    pub(crate) fn current(&self, window: Window, hash: u64) -> Option<u64> {
        self.sent
            .get(&window)
            .filter(|(sent, _)| *sent == hash)
            .map(|(_, generation)| *generation)
    }

    /// Assigns a fresh generation to `window`'s new content. Generations never repeat.
    pub(crate) fn record(&mut self, window: Window, hash: u64) -> u64 {
        self.last += 1;
        self.sent.insert(window, (hash, self.last));
        self.last
    }

    pub(crate) fn forget(&mut self, window: Window) {
        self.sent.remove(&window);
    }

    pub(crate) fn clear(&mut self) {
        self.sent.clear();
    }
}

/// Content hash of one preview's BGRX pixels and size.
pub(crate) fn pixel_hash(width: u32, height: u32, bgrx: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (width, height).hash(&mut hasher);
    bgrx.hash(&mut hasher);
    hasher.finish()
}

/// Pixels for references whose generation an unflushed signal still holds. The reactor keeps
/// only the latest signal, so a dropped pixel-bearing signal must not strand its references.
pub(crate) fn carry_pixels(previous: Option<&ControlSignal>, entries: &mut [SwitcherEntry]) {
    let Some(ControlSignal::WindowSwitcherChanged {
        entries: old_entries,
        ..
    }) = previous
    else {
        return;
    };
    let carried: HashMap<u64, &SwitcherThumb> = old_entries
        .iter()
        .filter_map(|entry| entry.thumb.as_ref())
        .filter(|thumb| thumb.generation != 0 && !thumb.is_reference())
        .map(|thumb| (thumb.generation, thumb))
        .collect();
    for entry in entries {
        if let Some(thumb) = entry.thumb.as_mut().filter(|t| t.is_reference())
            && let Some(full) = carried.get(&thumb.generation)
        {
            thumb.rgba = full.rgba.clone();
        }
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
            // Visible windows are re-read; minimised or other-workspace ones keep their last frame.
            let current = self.current_workspace;
            let clients = &self.clients;
            let hidden = |window: Window| {
                clients.get(&window).is_some_and(|state| {
                    state.minimized || (!state.sticky && state.workspace != current)
                })
            };
            self.switcher_thumbs.begin_session(hidden);
            // A new session starts with an empty shell cache: every preview goes out again once.
            self.switcher_previews.clear();
            let listed = self
                .switcher
                .view()
                .map(|(windows, _)| windows.to_vec())
                .unwrap_or_default();
            for window in listed {
                self.watch_switcher_damage(window);
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
    pub(super) fn publish_switcher(&mut self) {
        let immediate = self.switcher.immediate();
        let previous = self.switcher_signal.take();
        let Some((windows, selected)) = self.switcher.view().map(|(w, s)| (w.to_vec(), s)) else {
            self.switcher_previews.clear();
            self.switcher_signal = Some(closed_signal());
            return;
        };
        let mut entries = Vec::with_capacity(windows.len());
        for (index, window) in windows.iter().enumerate() {
            let mut entry = self.switcher_entry(*window);
            if index < MAX_THUMBS {
                entry.thumb = self.switcher_thumb(*window);
            }
            entries.push(entry);
        }
        carry_pixels(previous.as_ref(), &mut entries);
        self.switcher_signal = Some(ControlSignal::WindowSwitcherChanged {
            open: true,
            immediate,
            selected: u32::try_from(selected).unwrap_or(0),
            entries,
        });
    }

    fn switcher_entry(&self, window: Window) -> SwitcherEntry {
        match self.clients.get(&window) {
            Some(state) => SwitcherEntry {
                title: state.title.to_string(),
                app_id: state.wm_class.to_string(),
                thumb: None,
            },
            None => SwitcherEntry {
                title: String::new(),
                app_id: String::new(),
                thumb: None,
            },
        }
    }

    /// Ready preview for `window`, or `None` (and a capture is queued). Never reads pixels here:
    /// only the geometry round trip, which yields the cache key's size.
    fn switcher_thumb(&mut self, window: Window) -> Option<SwitcherThumb> {
        let geom = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let key = ThumbKey {
            window,
            src_w: u32::from(geom.width),
            src_h: u32::from(geom.height),
        };
        let Some((w, h, px)) = self.switcher_thumbs.request(key) else {
            self.switcher_previews.forget(window);
            return None;
        };
        let (w, h) = (*w, *h);
        let hash = pixel_hash(w, h, px);
        if let Some(generation) = self.switcher_previews.current(window, hash) {
            return Some(SwitcherThumb {
                width: w,
                height: h,
                generation,
                rgba: Vec::new(),
            });
        }
        let rgba = thumb_rgba(w, h, px)?;
        let generation = self.switcher_previews.record(window, hash);
        Some(SwitcherThumb {
            width: w,
            height: h,
            generation,
            rgba,
        })
    }

    /// Watches `window` for repaints while the switcher is open. Its DAMAGE events arrive on the
    /// WM's existing `DamageNotify` path (see `switcher_damaged`); no new event source.
    pub(super) fn watch_switcher_damage(&mut self, window: Window) {
        if self.switcher_damage.contains_key(&window) {
            return;
        }
        let Ok(id) = self.conn.generate_id() else {
            return;
        };
        let version_ok = damage::query_version(&self.conn, 1, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if version_ok
            && damage::create(&self.conn, id, window, damage::ReportLevel::NON_EMPTY).is_ok()
        {
            // Clear paints from before the watch: NON_EMPTY reports only an empty-to-damaged change.
            let _ = damage::subtract(&self.conn, id, 0u32, 0u32);
            self.switcher_damage.insert(window, id);
        }
    }

    /// A watched window repainted while the switcher is open: re-arms its damage object, drops
    /// the stale preview and republishes, so the open switcher recaptures it.
    pub(super) fn switcher_damaged(&mut self, window: Window, damage_id: u32) {
        // NON_EMPTY reports again only after the damage region is subtracted.
        let _ = damage::subtract(&self.conn, damage_id, 0u32, 0u32);
        self.switcher_thumbs.damage(window);
        if self.switcher.is_open() {
            self.publish_switcher();
        }
    }

    pub(super) fn release_switcher_damage(&mut self) {
        for (_, id) in self.switcher_damage.drain() {
            let _ = damage::destroy(&self.conn, id);
        }
    }

    pub(super) fn release_switcher_damage_for(&mut self, window: Window) {
        if let Some(id) = self.switcher_damage.remove(&window) {
            let _ = damage::destroy(&self.conn, id);
        }
    }

    /// Event-loop hook: runs up to `SWITCHER_CAPTURES_PER_TURN` captures while the switcher is
    /// open. Republishes the snapshot when a preview landed. Returns the thumbnails stored.
    pub(crate) fn pump_switcher_thumbs(&mut self) -> usize {
        if !self.switcher.is_open() {
            return 0;
        }
        self.switcher_budget.begin_turn();
        let clients = &self.clients;
        let iconic = |window: Window| clients.get(&window).is_some_and(|state| state.minimized);
        let redirect = self.redirect_frames;
        let redirected_frame = |window: Window| {
            clients
                .get(&window)
                .filter(|_| redirect)
                .map(|state| state.frame)
        };
        let mut source = X11ThumbSource {
            conn: self.conn,
            iconic: &iconic,
            redirected_frame: &redirected_frame,
        };
        let stored = self
            .switcher_thumbs
            .pump(&mut source, &mut self.switcher_budget);
        if stored > 0 {
            self.publish_switcher();
        }
        stored
    }

    /// True while an open switcher still has captures queued (the loop must not sleep).
    pub(crate) fn switcher_thumbs_pending(&self) -> bool {
        self.switcher.is_open() && self.switcher_thumbs.pending_len() > 0
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
        // Frames stay in the LRU-bounded provider so hidden windows keep their last frame.
        self.switcher_thumbs.end_session();
        self.release_switcher_damage();
        self.switcher_budget = CaptureBudget::new(SWITCHER_CAPTURES_PER_TURN);
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
    fn preview_ledger_references_unchanged_content_and_renews_on_change() {
        let mut ledger = PreviewLedger::default();
        let first = ledger.record(7, 0xAA);
        assert_eq!(
            ledger.current(7, 0xAA),
            Some(first),
            "same content is a reference"
        );
        assert_eq!(
            ledger.current(7, 0xBB),
            None,
            "changed content must be resent"
        );
        let second = ledger.record(7, 0xBB);
        assert_ne!(first, second, "generations never repeat");
        ledger.clear();
        assert_eq!(
            ledger.current(7, 0xBB),
            None,
            "a new session resends everything"
        );
    }

    #[test]
    fn carry_pixels_fills_references_from_the_unflushed_signal() {
        let pixels = SwitcherThumb {
            width: 1,
            height: 1,
            generation: 4,
            rgba: vec![9, 9, 9, 255],
        };
        let previous = ControlSignal::WindowSwitcherChanged {
            open: true,
            immediate: false,
            selected: 0,
            entries: vec![SwitcherEntry {
                title: String::new(),
                app_id: String::new(),
                thumb: Some(pixels.clone()),
            }],
        };
        let reference = SwitcherThumb {
            rgba: Vec::new(),
            ..pixels.clone()
        };
        let mut entries = vec![SwitcherEntry {
            title: String::new(),
            app_id: String::new(),
            thumb: Some(reference),
        }];
        carry_pixels(Some(&previous), &mut entries);
        assert_eq!(entries[0].thumb, Some(pixels));
        carry_pixels(None, &mut entries);
    }

    #[test]
    fn thumb_rgba_swaps_bgrx_to_opaque_rgba_and_checks_length() {
        // One BGRX pixel per entry: blue, green, red, padding.
        let bgrx = [1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(
            thumb_rgba(2, 1, &bgrx),
            Some(vec![3, 2, 1, 255, 6, 5, 4, 255])
        );
        assert_eq!(thumb_rgba(2, 2, &bgrx), None, "length must match w*h*4");
        assert_eq!(thumb_rgba(THUMB_MAX_W + 1, 1, &[0; 4]), None);
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
