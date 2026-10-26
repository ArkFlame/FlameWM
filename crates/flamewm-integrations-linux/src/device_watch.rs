//! Live device signal: kernel uevents for the block subsystem plus mount-table changes.
//! A helper thread blocks in `poll`; either source wakes the owner once the burst has been quiet for `QUIET`.
//! The explorer reloads its sidebar on the signal, and the session runs the removable remount pass on it.
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::net::netlink::{self, SocketAddrNetlink};
use rustix::net::{AddressFamily, RecvFlags, SocketFlags, SocketType, bind, recv, socket_with};

/// Udisks mounts a new disk after its uevents, so a burst of events settles before the owner reacts.
pub const QUIET: Duration = Duration::from_millis(300);
/// Netlink group the kernel multicasts uevents on (udev's rebroadcast group is 2 and is not needed).
const KERNEL_UEVENT_GROUP: u32 = 1;

/// True when a kernel uevent datagram belongs to the block subsystem: its NUL-separated fields include `SUBSYSTEM=block`.
#[must_use]
pub fn is_block_uevent(datagram: &[u8]) -> bool {
    datagram
        .split(|byte| *byte == 0)
        .any(|field| field == b"SUBSYSTEM=block")
}

fn uevent_socket() -> io::Result<OwnedFd> {
    let fd = socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        Some(netlink::KOBJECT_UEVENT),
    )?;
    bind(&fd, &SocketAddrNetlink::new(0, KERNEL_UEVENT_GROUP))?;
    Ok(fd)
}

/// Reads every queued uevent; returns true when one of them concerns a block device.
fn drain_uevents(socket: &OwnedFd, buffer: &mut [u8]) -> bool {
    let mut block = false;
    while let Ok((_, len)) = recv(socket, &mut buffer[..], RecvFlags::empty()) {
        block |= is_block_uevent(&buffer[..len]);
    }
    block
}

/// Owns the watch thread. Dropping it writes the stop pipe and joins the thread.
pub struct DeviceWatch {
    stop_tx: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl DeviceWatch {
    /// Without a uevent socket (no netlink in a sandbox), mount-table changes still signal.
    pub fn spawn(notify: impl Fn() + Send + 'static) -> io::Result<Self> {
        let uevents = uevent_socket().ok();
        let mounts = File::open("/proc/self/mountinfo")?;
        let (stop_rx, stop_tx) = rustix::pipe::pipe().map_err(io::Error::from)?;
        let thread = thread::Builder::new()
            .name("device-watch".into())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                let mut pending: Option<Instant> = None;
                loop {
                    let timeout = pending.map(|since| {
                        let left = QUIET.saturating_sub(since.elapsed());
                        Timespec {
                            tv_sec: left.as_secs() as _,
                            tv_nsec: left.subsec_nanos().into(),
                        }
                    });
                    let mut fds = vec![
                        PollFd::new(&stop_rx, PollFlags::IN),
                        PollFd::new(&mounts, PollFlags::PRI),
                    ];
                    if let Some(socket) = &uevents {
                        fds.push(PollFd::new(socket, PollFlags::IN));
                    }
                    match poll(&mut fds, timeout.as_ref()) {
                        Ok(_) => {}
                        Err(Errno::INTR) => continue,
                        Err(_) => break,
                    }
                    if fds[0]
                        .revents()
                        .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
                    {
                        break;
                    }
                    // The kernel flags a mount-table change as PRI/ERR on this fd; it is requested with PRI only.
                    if fds[1].revents().intersects(PollFlags::PRI | PollFlags::ERR) {
                        pending.get_or_insert_with(Instant::now);
                    }
                    if let (Some(socket), Some(ready)) = (&uevents, fds.get(2)) {
                        if ready.revents().intersects(PollFlags::IN)
                            && drain_uevents(socket, &mut buffer)
                        {
                            pending.get_or_insert_with(Instant::now);
                        }
                    }
                    if pending.is_some_and(|since| since.elapsed() >= QUIET) {
                        pending = None;
                        notify();
                    }
                }
            })?;
        Ok(Self {
            stop_tx,
            thread: Some(thread),
        })
    }
}

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        let _ = rustix::io::write(&self.stop_tx, &[1]);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_block_subsystem_uevents_wake_the_sidebar() {
        let block = b"add@/devices/pci0000:00/usb1/1-1/block/sdb\0ACTION=add\0DEVNAME=sdb\0SUBSYSTEM=block\0";
        let usb =
            b"add@/devices/pci0000:00/usb1/1-1\0ACTION=add\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0";
        assert!(is_block_uevent(block));
        assert!(!is_block_uevent(usb));
        assert!(!is_block_uevent(b"SUBSYSTEM=blockx\0"));
    }
}
