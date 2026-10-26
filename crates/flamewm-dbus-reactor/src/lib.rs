//! libdbus-backed, external-reactor D-Bus pump.
//!
//! `FlameWM` deliberately does not use `dbus-tokio` or a second event loop. The `FlameWM` desktop backend
//! registers `watch()` with its own `MainLoopPort`, then calls `on_ready()` when that fd fires.
//! After each turn it re-reads `watch()` and adjusts the engine registration if libdbus changed the
//! read/write interest.

use std::time::Duration;

use dbus::Message;
use dbus::channel::{BusType, Channel, Watch, default_reply};
use flamewm_api::ports::FdEvents;
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_integrations_core::dispatcher::MAX_DISPATCH_PER_TURN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    Session,
    System,
}

impl BusKind {
    const fn native(self) -> BusType {
        match self {
            Self::Session => BusType::Session,
            Self::System => BusType::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusWatch {
    pub fd: i32,
    pub events: FdEvents,
}

pub enum MessageDisposition {
    Handled,
    Reply(Message),
    Unhandled,
}

/// One libdbus connection whose I/O is driven by the engine's reactor.
pub struct BusPump {
    channel: Channel,
}

impl BusPump {
    /// Connection establishment may block briefly, matching libdbus semantics. Steady-state I/O is
    /// nonblocking and externally driven.
    pub fn connect(kind: BusKind) -> FlameResult<Self> {
        let mut channel = Channel::get_private(kind.native()).map_err(map_dbus_error)?;
        channel.set_watch_enabled(true);
        Ok(Self { channel })
    }

    #[must_use]
    pub fn unique_name(&self) -> Option<&str> {
        self.channel.unique_name()
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        watch_contract(self.channel.watch())
    }

    /// Nonblocking read/write + bounded raw message drain. The raw-visit budget is a hard CPU bound;
    /// unhandled messages count toward it just like useful messages.
    pub fn on_ready(
        &self,
        mut handler: impl FnMut(&Message) -> MessageDisposition,
    ) -> FlameResult<usize> {
        self.channel
            .read_write(Some(Duration::ZERO))
            .map_err(|()| FlameError::unavailable("D-Bus connection read/write failed"))?;

        let mut visited = 0;
        while visited < MAX_DISPATCH_PER_TURN {
            let Some(message) = self.channel.pop_message() else {
                break;
            };
            visited += 1;
            match handler(&message) {
                MessageDisposition::Handled => {}
                MessageDisposition::Reply(reply) => {
                    self.send(reply)?;
                }
                MessageDisposition::Unhandled => {
                    if let Some(reply) = default_reply(&message) {
                        self.channel.send(reply).map_err(|()| {
                            FlameError::new(ErrorCode::IoFailure, "D-Bus default reply send failed")
                        })?;
                    }
                }
            }
        }
        Ok(visited)
    }

    /// Blocks the calling thread until the bus fd is ready for read/write or `timeout` elapses. It
    /// does not dispatch: callers must follow with `on_ready` to drain queued messages.
    pub fn wait_ready(&self, timeout: Duration) -> FlameResult<()> {
        self.channel
            .read_write(Some(timeout))
            .map_err(|()| FlameError::unavailable("D-Bus connection read/write failed"))
    }

    /// Dispatch incoming method calls for one exported object path on this connection. Other
    /// messages remain available to the general handler, so one pump owns both client and server I/O.
    pub fn on_ready_with_object_path(
        &self,
        object_path: &str,
        mut object_handler: impl FnMut(&Message) -> MessageDisposition,
        mut handler: impl FnMut(&Message) -> MessageDisposition,
    ) -> FlameResult<usize> {
        self.on_ready(|message| {
            if message.msg_type() == dbus::MessageType::MethodCall
                && message.path().is_some_and(|path| path == object_path)
            {
                object_handler(message)
            } else {
                handler(message)
            }
        })
    }

    /// Queue an already-encoded D-Bus message. If libdbus cannot flush it immediately, `watch()`
    /// will expose WRITABLE interest and the engine adapter must update its poll registration.
    pub fn send(&self, message: Message) -> FlameResult<u32> {
        self.channel
            .send(message)
            .map_err(|()| FlameError::new(ErrorCode::IoFailure, "D-Bus message send failed"))
    }

    pub fn send_with_reply(&self, message: Message, timeout: Duration) -> FlameResult<Message> {
        self.channel
            .send_with_reply_and_block(message, timeout)
            .map_err(|error| FlameError::unavailable(format!("D-Bus reply failed: {error}")))
    }

    /// Claim one well-known name before exposing the server object.
    pub fn own_name(&self, name: &str) -> FlameResult<()> {
        let call = Message::new_method_call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
        )
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append2(name.to_owned(), 0x4_u32);
        let reply = self
            .channel
            .send_with_reply_and_block(call, Duration::from_secs(5))
            .map_err(|error| {
                FlameError::unavailable(format!("D-Bus name request failed: {error}"))
            })?;
        let result: u32 = reply.read1().map_err(|error| {
            FlameError::unavailable(format!("D-Bus name request reply invalid: {error}"))
        })?;
        if result != 1 {
            return Err(FlameError::unavailable(format!(
                "D-Bus name {name} unavailable (result {result})"
            )));
        }
        Ok(())
    }
}

#[must_use]
pub fn watch_contract(watch: Watch) -> BusWatch {
    let mut events = FdEvents::empty();
    if watch.read {
        events = events.union(FdEvents::READABLE);
    }
    if watch.write {
        events = events.union(FdEvents::WRITABLE);
    }
    // Error/Hangup are always useful even if libdbus currently requests no normal direction.
    events = events.union(FdEvents::ERROR).union(FdEvents::HANGUP);
    BusWatch {
        fd: watch.fd,
        events,
    }
}

fn map_dbus_error(error: dbus::Error) -> FlameError {
    FlameError::unavailable(format!("D-Bus connection failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_watch_maps_read_write_and_always_error_hangup() {
        let watch = watch_contract(Watch {
            fd: 7,
            read: true,
            write: false,
        });
        assert_eq!(watch.fd, 7);
        assert!(watch.events.contains(FdEvents::READABLE));
        assert!(!watch.events.contains(FdEvents::WRITABLE));
        assert!(watch.events.contains(FdEvents::ERROR));
        assert!(watch.events.contains(FdEvents::HANGUP));
    }

    #[test]
    fn system_and_session_bus_are_distinct_native_targets() {
        assert_ne!(BusKind::Session.native(), BusKind::System.native());
    }
}
