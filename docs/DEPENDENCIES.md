# Runtime dependencies

Table of record for what FlameWM needs at runtime and how each need is packaged.
`packaging/debian/control.in` and the `depends`/`optdepends` arrays in
`packaging/arch/PKGBUILD` (mirrored in `.SRCINFO`) must match the rows below.
`tools/test_flamewm_package_dependencies.py` enforces that.

## Evidence

- Shared-library requirements: `readelf -d` NEEDED entries of the release binaries
  in `target-rel/release` (flamewm-apps, flamewm-wayland, flamewm-lock, flamewm-session,
  flamewm-audio-helper, flamewm-render-x11, flamewm-render-compile,
  flamewm-render-inspect, flamewm-screenshot, flamefetch).
- Runtime-loaded libraries: `dlopen` names in `crates/**/*.rs` and sonames present
  in the binary strings. These are not in NEEDED, so the binary starts without them.
- External programs: every `Command::new("...")` under `crates/`. Programs used only by
  test harnesses (`crates/flamewm-x11-canary`, `crates/*/tests`) are not listed.
- Package names: `apt-cache policy` on Ubuntu 26.04 (Debian-derived) for every Debian
  name below. MX Linux 23 is Debian 12 (bookworm); bookworm itself was not available
  locally, so bookworm names are not separately verified. Arch names are not verified
  (no pacman here).

## Classes

- **Depends**: a shipped binary links it (NEEDED), or a shipped session cannot start
  without it.
- **Recommends**: an optional feature that is installed by default on Debian. Light
  packages only.
- **Suggests**: a heavy or niche optional tool. Not installed by default. Arch has no
  Recommends, so Recommends and Suggests both become `optdepends`.

## Table

| Debian/Ubuntu package | Arch package | Class | Needed by | Without it |
| --- | --- | --- | --- | --- |
| libc6 | glibc | Depends | Every ELF binary (NEEDED libc.so.6) | Binaries do not start |
| libgcc-s1 | libgcc | Depends | Every ELF binary (NEEDED libgcc_s.so.1) | Binaries do not start |
| libx11-6 | libx11 | Depends | flamewm-apps, flamewm-lock, flamewm-render-x11 (NEEDED) | Binaries do not start |
| libxi6 | libxi | Depends | flamewm-apps, flamewm-lock, flamewm-render-x11 (NEEDED, XInput2) | Binaries do not start |
| libxkbcommon0 | libxkbcommon | Depends | flamewm-wayland (NEEDED) | Wayland binary does not start |
| libdbus-1-3 | dbus | Depends | flamewm-apps, flamewm-session, flamewm-lock, flamewm-wayland (NEEDED); control bus | Binaries do not start |
| libpulse0 | libpulse | Depends | flamewm-audio-helper (NEEDED) | Audio helper does not start |
| libpam0g | pam | Depends | flamewm-lock loads libpam.so.0 | Lock screen cannot authenticate |
| libpam-runtime | pambase | Depends | packaging/pam.d/flamewm-lock includes common-auth and common-account | Lock screen cannot authenticate |
| libxft2 | libxft | Depends | flamewm-render-x11 text (Xft loads on first text draw) | Text falls back to core X11 fonts (degraded, not fatal) |
| libfontconfig1 | fontconfig | Depends | Xft font discovery; flamewm-lock and flamewm-render-x11 load libfontconfig | Same fallback as libxft2 |
| libgbm1 | mesa | Depends | flamewm-wayland (NEEDED); DRM backend buffers | Wayland binary does not start |
| libdrm2 | libdrm | Depends | flamewm-wayland DRM backend (dlopen libdrm.so.2; probe requires it for native) | DRM Wayland session fails its probe |
| libinput10 | libinput | Depends | flamewm-wayland (NEEDED); native input devices | Wayland binary does not start |
| libudev1 | systemd-libs | Depends | flamewm-wayland (NEEDED); device enumeration | Wayland binary does not start |
| libegl1 | libglvnd | Depends | Wayland EGL renderer (probe requires it on both backends); flamewm-apps GPU blur (dlopen, CPU fallback) | Wayland session fails its probe; X11 uses CPU blur |
| libgles2 | libglvnd | Depends | Wayland GLES renderer (probe requires it on both backends); flamewm-apps GPU blur (dlopen, CPU fallback) | Wayland session fails its probe; X11 uses CPU blur |
| python3 | python | Depends | packaging/libexec flamewm-desktop-services (autostart, polkit, notifications bridge) | Session starts without the services bridge (degraded) |
| xwayland | xorg-xwayland | Depends | flamewm-wayland spawns Xwayland for X11 clients (probe requires it) | Wayland session fails its probe; X11 session unaffected |
| libxcursor1 | libxcursor | Recommends | Cursor theme loading (dlopen, `.ok()` in native/cursor.rs) | Cursor theme loading disabled |
| libxext6 | libxext | Recommends | XShape window masks (dlopen; rectangle fallback) | Rectangle masks instead of shaped windows |
| libxrender1 | libxrender | Recommends | Render extension (dlopen; core fallback) | Core rendering path |
| libxcomposite1 | libxcomposite | Recommends | Live backdrop for glass blur (optional `Option`) | No live backdrop |
| libxdamage1 | libxdamage | Recommends | Live backdrop damage tracking (with libxcomposite) | No live backdrop |
| libx11-xcb1 | libx11 | Recommends | Nested X11 host backend (flamewm-wayland dlopen) | Nested X11 backend unavailable |
| libwayland-client0 | wayland | Recommends | Nested Wayland host backend (flamewm-wayland dlopen) | Nested Wayland backend unavailable |
| libwayland-egl1 | wayland | Recommends | Nested Wayland host backend EGL window | Nested Wayland backend unavailable |
| xdg-utils | xdg-utils | Recommends | xdg-open and xdg-mime (Explorer, portal file and URL opening) | Files and links are not opened by default apps |
| libnotify-bin | libnotify | Recommends | notify-send (Explorer notifications) | Notifications silently skipped |
| dunst | dunst | Recommends | Notification daemon; bundled packaging/notifications/dunstrc | No notification popups (another daemon can be used) |
| fontconfig | - | Recommends | fc-list font enumeration (covered on Arch by libfontconfig1 to fontconfig) | Font enumeration unavailable |
| systemd | systemd | Recommends | systemd-logind seat for the Wayland DRM session; busctl, loginctl, systemctl (reboot and poweroff). elogind also provides logind | DRM Wayland session cannot take the seat; power actions fail |
| xdg-desktop-portal | xdg-desktop-portal | Recommends | Portal frontend for packaging/xdg-desktop-portal/flamewm-portals.conf | No portal dialogs |
| xdg-desktop-portal-gtk | xdg-desktop-portal-gtk | Recommends | Default portal backend (`default=gtk`) | No GTK portal backend |
| x11-xserver-utils | xorg-xrandr | Suggests | xrandr layout query and restore at login (flamewm-x11-desktop, flamewm-wm) | Display layout not restored |
| x11-utils | xorg-xprop | Suggests | xprop readiness gate for deferred autostart (tools/flamewm_desktop_services.py) | Autostart gate degrades |
| dex | dex | Suggests | `dex --autostart` for XDG autostart entries | XDG autostart entries do not run |
| ffmpeg | ffmpeg | Suggests | Screen recording (flamewm-screenshot, flamewm-explorer) | Recording disabled |
| imagemagick | imagemagick | Suggests | `convert` and `import` (Convert wheel, screenshot tools) | Convert wheel disabled |
| ark | ark | Suggests | Archive extraction from Explorer | Extraction disabled |
| zip | zip | Suggests | Archive creation (flamewm-desktop-core conversion) | Zip creation disabled |
| curl | curl | Suggests | Weather data fetch (flamewm-shell weather_provider) | Weather widget has no data |
| udisks2 | udisks2 | Suggests | udisksctl mount of removable drives (Explorer) | Mount disabled |
| policykit-1-gnome | polkit-gnome | Suggests | Polkit authentication agent fallback | Privileged actions have no prompt agent |
| polkit-kde-agent-1 | polkit-kde-agent | Suggests | Preferred polkit authentication agent | Privileged actions have no prompt agent |
| xdg-desktop-portal-kde | xdg-desktop-portal-kde | Suggests | Preferred OpenURI and FileChooser backend | GTK backend is used |
| libxkbcommon-x11-0 | libxkbcommon-x11 | Suggests | Nested X11 keyboard support in flamewm-wayland (dlopen) | Nested X11 backend keyboard fails |
| fonts-ibm-plex | ttf-ibm-plex | Suggests | Default interface font | Falls back to Noto Sans or DejaVu Sans |

## Deliberately not declared

- **X server (xserver-xorg, Xorg) and xinit**: no shipped binary runs `startx` or
  `xinit`. The X11 session is launched by a display manager, which supplies the X
  server. The package does not depend on a display manager.
- **Xvfb (xvfb)**: used only by `crates/*/tests`. Not runtime.
- **pw-cli (pipewire-bin), tesseract (tesseract-ocr)**: used only by
  `crates/flamewm-x11-canary` (test harness). Not runtime.
- **libseat**: probed as informational only; seat access uses systemd-logind over D-Bus.
- **kwallet-pam and KWallet packages**: a display-manager PAM concern, not a FlameWM binary
  requirement.
- **Essential or base utilities**: bash, coreutils, util-linux (lsblk, setsid, renice),
  procps (pgrep), tar, xz-utils, sh, env, sleep, kill. Present on every base install.
- **dbus-bin**: provides dbus-update-activation-environment. Used by the session wrapper,
  which degrades portal environment integration without it. Not declared (dbus-daemon
  installs the bus; dependency relation not verified here).
- **Desktop applications launched by the user** (dolphin, konsole, flamewm-finder): chosen
  by the user, not dependencies.

## Session registration

- `/usr/share/xsessions/flamewm.desktop`: `Exec=/usr/libexec/flamewm/flamewm-session-wrapper`,
  `TryExec=/usr/bin/flamewm-session`. X11 session; the display manager supplies the X server.
- `/usr/share/wayland-sessions/flamewm-wayland.desktop`: `Exec=/usr/libexec/flamewm/flamewm-wayland-session --backend=drm`,
  `TryExec=/usr/bin/flamewm-wayland`. Needs Xwayland, EGL and GLES, and systemd-logind.

Both entries point at files that `scripts/package` installs. No dependency is needed
to register them, beyond the display manager that lists wayland-sessions and xsessions.

## Linked libraries

Before-set: `readelf -d` NEEDED on the existing `target-rel/release` binaries, which predate the libXi change (sizes in bytes).

| Binary | Size | NEEDED before |
| --- | --- | --- |
| flamewm | 4966160 | libX11, libdbus-1, libgcc_s, libm, libc |
| flamewm-desktop | 9398784 | libX11, libXi, libdbus-1, libgcc_s, libm, libc |
| flamewm-apps | 28709288 | libX11, libXi, libdbus-1, libgcc_s, libm, libc |
| flamewm-settings | 12309704 | libX11, libXi, libdbus-1, libgcc_s, libm, libc |
| flamewm-explorer | 4658448 | libdbus-1, libX11, libXi, libgcc_s, libm, libc |
| flamewm-finder | 2970704 | libX11, libXi, libdbus-1, libgcc_s, libm, libc |
| flamewm-lock | 3613864 | libX11, libdbus-1, libXi, libgcc_s, libm, libc |
| flamewm-session | 871800 | libdbus-1, libgcc_s, libc |
| flamewm-audio-helper | 443288 | libpulse, libgcc_s, libc |
| flamewm-render-x11 | 2372640 | libX11, libXi, libgcc_s, libm, libc |
| flamefetch | 1819416 | libgcc_s, libm, libc |

After-set: not rebuilt. The libXi removal is verified only by `cargo test -p flamewm-render-x11 --lib raw_press` (5 passed). Expected effect is removal of libXi from the render-x11 dependents; confirm with a release build and `readelf -d`.

### Ranking (install risk on Debian/Ubuntu/MX desktop x ease of replacement)

| Rank | Library / executable | Where | Risk | Ease | Decision |
| --- | --- | --- | --- | --- | --- |
| 1 | libXi | render-x11 XInput2 | low | easy | dlopen'd (commit 82528fb2) |
| 2 | libpulse | audio-helper only | low | n/a | already isolated from the WM |
| 3 | libdbus-1 | control bus via `dbus` 0.9.12 in about a dozen crates | very low | high | kept: core IPC transport |
| 4 | libX11 | Xlib FFI boundary for all GUI binaries | none on X desktops | high | kept |
| 5 | libc, libm, libgcc_s | all | none | n/a | kept |

External executables (`Command::new`) are runtime tools (xrandr, xdg-open, systemctl, loginctl, udisksctl, fc-list, ffmpeg, xz) and are not replaced.

### libXi change

`XIQueryVersion` and `XISelectEvents` are resolved with `DynamicLibrary` on first use and the library stays mapped for process lifetime. If libXi is absent, `FLAMEWM_XI unavailable` is logged once and raw-press dismissal is disabled. FocusOut and Escape still dismiss popups.

### Intentionally kept NEEDED libraries

libX11, libdbus-1, libc, libm and libgcc_s stay linked. Each is either the X11 session boundary, the core IPC transport, or part of the base C runtime, and replacing any of them is a multi-crate rewrite with no install-risk gain on the target desktops.
