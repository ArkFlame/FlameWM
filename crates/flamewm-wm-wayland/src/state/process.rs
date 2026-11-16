use super::*;

impl State {
    /// Starts a program. Once `XWayland` runs, children get `DISPLAY` and no `WAYLAND_DISPLAY`,
    /// so toolkits use X11 and reach the compositor through `XWayland`.
    pub fn spawn(&self, program: &str, args: &[&str]) {
        let mut command = Command::new(program);
        command.args(args);
        match self.display_number {
            Some(display) => {
                command
                    .env("DISPLAY", format!(":{display}"))
                    .env_remove("WAYLAND_DISPLAY")
                    .env("GDK_BACKEND", "x11")
                    .env("QT_QPA_PLATFORM", "xcb")
                    .env("SDL_VIDEODRIVER", "x11")
                    .env("MOZ_ENABLE_WAYLAND", "0");
            }
            None => {
                command.env("WAYLAND_DISPLAY", &self.socket_name);
            }
        }
        match command.spawn() {
            Ok(child) => {
                self.watch_exit(&child);
                self.children.borrow_mut().push(child);
            }
            Err(error) => eprintln!("flamewm-wayland: spawn {program} failed: {error}"),
        }
    }

    /// Wakes the loop once when `child` exits so it is reaped then; nothing polls for exits.
    fn watch_exit(&self, child: &Child) {
        let pid = rustix::process::Pid::from_child(child);
        let watched = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
            .map_err(|error| error.to_string())
            .and_then(|pidfd| {
                self.loop_handle
                    .insert_source(
                        Generic::new(pidfd, Interest::READ, CalloopMode::Level),
                        |_, _, state| {
                            state.reap_children();
                            Ok(PostAction::Remove)
                        },
                    )
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = watched {
            eprintln!("flamewm-wayland: child {pid:?} exit is not watched: {error}");
        }
    }

    /// Emits a `com.arkflame.FlameWM1` signal on the session bus, the same signal the X11 WM sends.
    pub fn emit_control_signal(&self, member: &str) {
        let interface = format!("com.arkflame.FlameWM1.{member}");
        self.spawn(
            "dbus-send",
            &[
                "--session",
                "--type=signal",
                "/com/arkflame/FlameWM1",
                interface.as_str(),
            ],
        );
    }
}
