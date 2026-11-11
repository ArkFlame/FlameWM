//! `org.freedesktop.impl.portal.ScreenCast` surface of the `FlameWM` portal backend.
//!
//! Precedent: `.vendor/cosmic-epoch/xdg-desktop-portal-cosmic/src/screencast.rs` serves the same
//! calls (`CreateSession`, `SelectSources`, `Start`) and streams frames through `PipeWire`. `FlameWM` has no
//! `PipeWire` stream owner yet (`docs/backlog/remaining-audit.md`, decision D-portal), so this module
//! registers the interface and answers every call with response 2 (other failure) and empty
//! results. No session or stream is ever created, and the frontend gets a prompt, honest error
//! instead of a timeout.

pub const INTERFACE: &str = "org.freedesktop.impl.portal.ScreenCast";
pub const MEMBERS: [&str; 3] = ["CreateSession", "SelectSources", "Start"];
pub const REFUSAL: &str = "ScreenCast is not supported: no PipeWire stream owner in FlameWM";

/// True when `(interface, member)` is one of the `ScreenCast` calls this backend answers.
pub fn is_call(interface: &str, member: &str) -> bool {
    interface == INTERFACE && MEMBERS.contains(&member)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal;
    use flamewm_dbus_reactor::MessageItem;

    #[test]
    fn every_screencast_call_is_recognised() {
        for member in MEMBERS {
            assert!(is_call(INTERFACE, member), "{member} must be served");
        }
    }

    #[test]
    fn screenshot_and_unknown_calls_are_not_screencast() {
        assert!(!is_call(
            "org.freedesktop.impl.portal.Screenshot",
            "Screenshot"
        ));
        assert!(!is_call(INTERFACE, "OpenPipeWireRemote"));
        assert!(!is_call("org.freedesktop.impl.portal.ScreenCastX", "Start"));
    }

    #[test]
    fn refusal_is_response_two_without_streams() {
        let items = portal::reply_items(portal::RESPONSE_OTHER, None).unwrap();
        assert_eq!(items[0], MessageItem::UInt32(2));
    }
}
