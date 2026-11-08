//! Sticky keys: a modifier tapped once latches for the next key press; tapped twice it locks.
//!
//! Pure state machine. It holds no I/O, timers or allocation. Callers feed every key event through
//! [`StickyKeys::on_key`] and forward the returned event. With the feature disabled the input passes
//! through unchanged.

/// Modifier bit flags carried on a [`KeyEvent`].
pub const SHIFT: u8 = 1 << 0;
pub const CTRL: u8 = 1 << 1;
pub const ALT: u8 = 1 << 2;
pub const SUPER: u8 = 1 << 3;

/// A latchable modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Super,
}

impl Modifier {
    /// The modifier bit this key contributes to [`KeyEvent::mods`].
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::Shift => SHIFT,
            Self::Ctrl => CTRL,
            Self::Alt => ALT,
            Self::Super => SUPER,
        }
    }
}

/// What a key is for latching purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Modifier(Modifier),
    Other,
}

/// One key event, reduced to what sticky keys needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub pressed: bool,
    /// Modifier bits held by the real keyboard (`SHIFT | CTRL | ALT | SUPER`).
    pub mods: u8,
}

/// Latch state. At most one modifier is latched at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Latch {
    Idle,
    /// Applies to the next non-modifier key press, then clears.
    Armed(Modifier),
    /// Applies to every non-modifier key press until tapped again.
    Locked(Modifier),
}

/// Sticky-keys state. Default is idle and disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StickyKeys {
    state: Latch,
    enabled: bool,
}

impl Default for StickyKeys {
    fn default() -> Self {
        Self::new(false)
    }
}

impl StickyKeys {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self {
            state: Latch::Idle,
            enabled,
        }
    }

    /// Enables or disables the feature. Disabling clears any latch.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.state = Latch::Idle;
        }
    }

    #[must_use]
    pub const fn latch(&self) -> Latch {
        self.state
    }

    /// Processes one key event and returns the event to forward.
    ///
    /// - A modifier press arms it. A second press of the same modifier locks it. Another press of a
    ///   locked modifier releases the lock.
    /// - A non-modifier key press gets the latched modifier bit. An armed latch then clears; a locked
    ///   latch stays.
    /// - Releases and disabled state pass the event through unchanged.
    pub fn on_key(&mut self, event: KeyEvent) -> KeyEvent {
        if !self.enabled || !event.pressed {
            return event;
        }
        match event.key {
            Key::Modifier(modifier) => {
                self.state = match self.state {
                    Latch::Locked(held) if held == modifier => Latch::Idle,
                    Latch::Armed(held) if held == modifier => Latch::Locked(modifier),
                    _ => Latch::Armed(modifier),
                };
                event
            }
            Key::Other => match self.state {
                Latch::Idle => event,
                Latch::Armed(modifier) => {
                    self.state = Latch::Idle;
                    KeyEvent {
                        mods: event.mods | modifier.bit(),
                        ..event
                    }
                }
                Latch::Locked(modifier) => KeyEvent {
                    mods: event.mods | modifier.bit(),
                    ..event
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            pressed: true,
            mods: 0,
        }
    }

    fn other() -> KeyEvent {
        press(Key::Other)
    }

    fn ctrl() -> KeyEvent {
        press(Key::Modifier(Modifier::Ctrl))
    }

    #[test]
    fn default_is_disabled_and_idle() {
        let sticky = StickyKeys::default();
        assert_eq!(sticky.latch(), Latch::Idle);
        assert_eq!(sticky, StickyKeys::new(false));
    }

    #[test]
    fn sticky_keys_shift_latches_for_one_key() {
        let mut sticky = StickyKeys::new(true);
        let shift = press(Key::Modifier(Modifier::Shift));
        assert_eq!(sticky.on_key(shift), shift);
        assert_eq!(sticky.latch(), Latch::Armed(Modifier::Shift));

        let out = sticky.on_key(other());
        assert_eq!(out.mods, SHIFT, "the next key carries Shift");
        assert_eq!(
            sticky.latch(),
            Latch::Idle,
            "Shift is released after one key"
        );

        assert_eq!(sticky.on_key(other()).mods, 0, "following key is plain");
    }

    #[test]
    fn sticky_keys_double_tap_ctrl_locks_until_tapped_again() {
        let mut sticky = StickyKeys::new(true);
        sticky.on_key(ctrl());
        sticky.on_key(ctrl());
        assert_eq!(sticky.latch(), Latch::Locked(Modifier::Ctrl));

        assert_eq!(sticky.on_key(other()).mods, CTRL, "c carries Ctrl");
        assert_eq!(sticky.on_key(other()).mods, CTRL, "v still carries Ctrl");
        assert_eq!(sticky.latch(), Latch::Locked(Modifier::Ctrl));

        sticky.on_key(ctrl());
        assert_eq!(sticky.latch(), Latch::Idle, "third Ctrl tap releases");
        assert_eq!(sticky.on_key(other()).mods, 0);
    }

    #[test]
    fn sticky_keys_releases_and_non_modifiers_do_not_change_latch() {
        let mut sticky = StickyKeys::new(true);
        sticky.on_key(ctrl());
        let release = KeyEvent {
            key: Key::Modifier(Modifier::Ctrl),
            pressed: false,
            mods: 0,
        };
        assert_eq!(sticky.on_key(release), release);
        assert_eq!(sticky.latch(), Latch::Armed(Modifier::Ctrl));
    }

    #[test]
    fn sticky_keys_disabled_is_a_no_op() {
        let mut sticky = StickyKeys::new(false);
        let shift = press(Key::Modifier(Modifier::Shift));
        assert_eq!(sticky.on_key(shift), shift);
        assert_eq!(sticky.on_key(shift), shift);
        assert_eq!(sticky.latch(), Latch::Idle);
        assert_eq!(sticky.on_key(other()), other());
    }

    #[test]
    fn sticky_keys_disabling_clears_latch() {
        let mut sticky = StickyKeys::new(true);
        sticky.on_key(ctrl());
        sticky.set_enabled(false);
        assert_eq!(sticky.latch(), Latch::Idle);
        assert_eq!(sticky.on_key(other()).mods, 0);
    }
}
