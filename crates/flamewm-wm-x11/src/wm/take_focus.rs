//! ICCCM 4.1.7 input focus delivery: whether a focus request calls `SetInputFocus` and whether it
//! sends `WM_TAKE_FOCUS`, from the client's `WM_HINTS` input flag and its `WM_PROTOCOLS` list.

use super::*;

/// What one focus request delivers to a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FocusDelivery {
    pub set_input_focus: bool,
    pub send_take_focus: bool,
}

/// `input` is the `WM_HINTS` input flag: `None` when absent or unreadable (ICCCM: absent means
/// false). A client that listens for `WM_TAKE_FOCUS` (globally active or no-input) gets the
/// protocol message and no WM `SetInputFocus` unless it declares input; it focuses itself.
pub(super) fn focus_delivery(input: Option<bool>, has_take_focus: bool) -> FocusDelivery {
    FocusDelivery {
        set_input_focus: input == Some(true) || !has_take_focus,
        send_take_focus: has_take_focus,
    }
}

/// `WM_HINTS` flags bit 0 is `InputHint`; the input value is the second 32-bit field.
pub(super) fn parse_input_hint(values: &[u32]) -> Option<bool> {
    const INPUT_HINT: u32 = 1;
    let flags = *values.first()?;
    if flags & INPUT_HINT == 0 {
        return None;
    }
    Some(values.get(1).copied().unwrap_or(0) != 0)
}

impl<C: Connection> Wm<'_, C> {
    pub(super) fn read_input_hint(&self, client: Window) -> Option<bool> {
        let reply = self
            .conn
            .get_property(false, client, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, 0, 9)
            .ok()?
            .reply()
            .ok()?;
        let values: Vec<u32> = reply.value32()?.collect();
        parse_input_hint(&values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_focus_protocol_suppresses_wm_set_focus_unless_input_declared() {
        // No-input client with WM_TAKE_FOCUS: protocol only.
        assert_eq!(
            focus_delivery(Some(false), true),
            FocusDelivery {
                set_input_focus: false,
                send_take_focus: true
            }
        );
        // Globally active (input + WM_TAKE_FOCUS): both.
        assert_eq!(
            focus_delivery(Some(true), true),
            FocusDelivery {
                set_input_focus: true,
                send_take_focus: true
            }
        );
        // Locally active / passive: no protocol, WM focuses as before.
        assert_eq!(
            focus_delivery(Some(true), false),
            FocusDelivery {
                set_input_focus: true,
                send_take_focus: false
            }
        );
        assert_eq!(
            focus_delivery(None, false),
            FocusDelivery {
                set_input_focus: true,
                send_take_focus: false
            }
        );
    }

    #[test]
    fn wm_hints_input_flag_is_read_only_when_declared() {
        assert_eq!(parse_input_hint(&[1, 1]), Some(true));
        assert_eq!(parse_input_hint(&[1, 0]), Some(false));
        assert_eq!(parse_input_hint(&[0, 1]), None);
        assert_eq!(parse_input_hint(&[1]), Some(false));
        assert_eq!(parse_input_hint(&[]), None);
    }
}
