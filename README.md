![FlameWM](assets/branding/flamewm-wordmark.png)

![FlameWM preview](assets/branding/flamewm-real-screenshot-preview.webp)

# FlameWM

FlameWM is a clean-room window manager and desktop shell written in Rust. The shell UI is authored as HTML/CSS and compiled at build time into native Rust rendering, with no browser runtime.

One package ships two sessions: **FlameWM** (X11, the supported default) and **FlameWM (Wayland)**, a preliminary compositor session that has not been validated on real hardware yet. See [Wayland session (preliminary)](#wayland-session-preliminary).

Version and release history: [`Cargo.toml`](Cargo.toml) (`workspace.package.version`), and [`CHANGELOG.md`](CHANGELOG.md).

## Status

- Source-level release. Remaining product surfaces are tracked in [`docs/TODO.md`](docs/TODO.md).
- FlameWM does not claim parity with the historical IceWM or FlameWM C implementations.
- Memory has been measured on a development machine only; see [Memory](#memory) for exactly what was and was not measured. The performance contract is in [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

## Wayland session (preliminary)

The `.deb` installs two session entries in one package. The X11 session stays the supported
default; the Wayland session is **preliminary and experimental**, and it has not been validated on
real hardware.

| Session | Session entry | Started by | Status |
| --- | --- | --- | --- |
| `FlameWM` (X11) | `/usr/share/xsessions/flamewm.desktop` | `flamewm-session-wrapper` on Xorg | Stable, recommended default |
| `FlameWM (Wayland)` | `/usr/share/wayland-sessions/flamewm-wayland.desktop` | DRM/KMS (native) or nested winit | Preliminary, unvalidated on hardware |

The session entry runs
`/usr/libexec/flamewm/flamewm-wayland-session --backend=drm`; the wrapper starts
`/usr/bin/flamewm-wayland` and hard-gates the native path on `flamewm-wayland --capabilities`
reporting `drm_compiled=true`. The compositor is `crates/flamewm-wm-wayland` (Smithay 0.7):

- Custom systemd-logind seat session (no libseat, no seatd): `TakeControl`/`TakeDevice`,
  `PauseDeviceComplete` only after the compositor quiesces, `ResumeDevice` descriptor handling.
- DRM/KMS through Smithay `DrmCompositor` (GBM + EGL/GLES), libinput, Ctrl+Alt+F1..F12 VT switch
  and resume.
- Dirty-driven frame scheduling: a redraw request is drawn or deferred to the queued frame, never
  painted by an idle timer.
- XWayland, so the existing shell, desktop, launcher and settings X11 clients run unchanged. Native
  Wayland shell clients do not exist yet.
- Nested developer backend: `--backend=nested` opens a winit window inside an existing Wayland or
  X11 session (output transform `Flipped180`, window sized to the host, host cursor). This is a
  developer mode only, not a product session.

Runtime dependencies added by the package on top of the X11 stack: `libgbm1`, `libdrm2`,
`libinput10`, `libudev1`, `libegl1`, `libgles2`, `xwayland`.

Install and select the Wayland session (read [Current limitations](#wayland-current-limitations)
first; install from a working X11 session so recovery stays available):

```bash
sudo dpkg -i .target/dist/flamewm_<version>_amd64.deb
flamewm-wayland --capabilities      # must print drm_compiled=true
```

Then log out, choose **FlameWM (Wayland)** in the display manager (LightDM), and log in. To verify a
successful start: first frame on the connector, panel and shell visible, a terminal opens and
accepts input, pointer and keyboard work, Ctrl+Alt+Fn switches away and back, and a second login
after a VT return works.

Diagnostics when the session does not come up:

```bash
flamewm-wayland --capabilities       # build facts only: no bus, no /dev, no seat access
flamewm-wayland --probe              # read-only advisory report; never gates startup
tail -n 200 ~/.cache/flamewm/wayland-session.log   # 0600, previous run kept as .log.1
journalctl -b                        # logind and DRM side of the same boot
```

The wrapper and the compositor both emit `FLAMEWM-WAYLAND-STARTUP stage=... status=... code=...`
lines, so the failing stage is named in the log. `--probe` records `[FAIL]` lines but never changes
the session exit code.

**Recovery:** choose **FlameWM** (X11) at the login screen. The package ships both sessions, and the
Wayland wrapper does not fall back to X11 by itself.

### Wayland current limitations

- Not validated on real hardware: no successful native DRM/LightDM login has been recorded. A
  reported failed login (text cursor, then the greeter returns) is under investigation and is not
  fixed. Treat the session as experimental and keep the X11 session as your daily driver.
- The shell, desktop, launcher, settings and screenshot clients are still X11 programs running on
  XWayland; layer-shell support exists in the compositor but nothing shipped uses it yet.
- No session lock, no screenshot/portal integration, no portal-driven capture.
- One connected connector drives the scene; further connectors show the same scene. No output
  layout, per-output scale, gamma or DPMS. The cursor is a software square, not a hardware plane.
- The terminal body stays black at the default 80% opacity because the translucent fill blends over
  a backdrop taken from the X root; at `background_opacity = 100` it renders correctly.
- Launcher settings parity with X11 is partial, and resize parity (edge-specific resize, Ctrl+Super
  drag) and taskbar middle-click have no automated coverage.

`docs/WAYLAND_PLAN.md` is the detailed implementation and status record.

## Features

- X11 window manager with reparenting, focus and activation, minimize/maximize/fullscreen, interactive move and resize, and snap previews (`crates/flamewm-wm`, `crates/flamewm-wm-x11`).
- Preliminary Wayland compositor session (`crates/flamewm-wm-wayland`): native DRM/KMS backend and a nested winit developer backend, with XWayland for the existing shell and desktop clients. Not yet validated on hardware; the X11 session remains the default.
- Virtual workspaces with keyboard switching.
- Desktop with icons, context menus, and wallpaper loading (`crates/flamewm-desktop`, `crates/flamewm-integrations-linux`).
- Panel, Start menu, and status popovers rendered by the shell (`crates/flamewm-shell`).
- Settings application with typed settings transactions across the platform services (`crates/flamewm-settings`, `crates/flamewm-platform`).
- Lock screen with PAM authentication (`crates/flamewm-lock`, `packaging/pam.d/flamewm-lock`).
- Native FlameWM Screenshot tool: X11 full/monitor/window/region capture, PNG saving, image/png clipboard, interactive pen/shape/arrow/redaction annotations, and optional ffmpeg screen recording (`crates/flamewm-screenshot`).
- Volume on-screen display driven by the XF86 audio keys.
- Weather popup with Open-Meteo forecast and location search. Requires network access; the Open-Meteo free tier is for noncommercial use only; attribution is shown in the popup. Disabled by default.

## X11 session limitations

- The X11 session is the supported default. The Wayland session is preliminary; see [Wayland session (preliminary)](#wayland-session-preliminary).
- Requires X11, Xft, and Fontconfig runtime libraries.
- The renderer crate (`crates/flamewm-render-x11`) is the explicit Xlib/Xft FFI boundary; the window manager itself uses safe `x11rb` protocol APIs.
- FlameWM does not replace a display manager. Select it from the session chooser after installation.
- Multi-output (two-output RandR) layouts have not been tested.
- Wayland background: [`docs/WAYLAND_PLAN.md`](docs/WAYLAND_PLAN.md) describes the implementation that ships and its unverified areas. [`docs/WAYLAND_BACKEND_FEASIBILITY.md`](docs/WAYLAND_BACKEND_FEASIBILITY.md) is the historical pre-implementation analysis and is superseded by that plan.

## Default shortcuts

The defaults below come from `crates/flamewm-platform/src/shortcuts.rs` (`ShortcutRegistry::default`). Bindings are configurable through settings.

| Action | Default |
| --- | --- |
| Toggle Start menu | `Super_L` (left Super, bare tap) |
| Switch workspace left / right | `Ctrl+Super+Left` / `Ctrl+Super+Right` |
| Switch workspace up / down | `Ctrl+Super+Up` / `Ctrl+Super+Down` |
| Close window | `Alt+F4` |
| Lock session | `Super+L` |
| All screens screenshot | `Print` |
| Capture and annotate region | `Shift+Print` |
| Active window screenshot | `Ctrl+Print` |
| Volume up / down / mute | `XF86AudioRaiseVolume` / `XF86AudioLowerVolume` / `XF86AudioMute` |
| Minimize / maximize window | unassigned |

## Screenshots and screen recording

Screenshots launch on demand; there is no always-running capture daemon. Use the
**FlameWM Screenshot** launcher or the default shortcuts above. PNGs are saved
under the XDG Pictures directory in `Screenshots/` (or `~/Pictures/Screenshots`)
and copied to CLIPBOARD when invoked from a hotkey. Saving never overwrites an
existing path. The clipboard owner remains idle and exits automatically when
another program replaces the clipboard.

```bash
flamewm-screenshot --region --annotate --copy
flamewm-screenshot --full --output ~/Pictures/desktop.png
flamewm-screenshot --screen --no-save --copy
flamewm-screenshot --window --delay 2 --copy
flamewm-screenshot --region --record 15 --fps 30
```

The native annotation toolbar supports `P` freehand, `L` line, `R` rectangle,
`E` ellipse, `A` arrow, `X` black redaction, `C` cycle color, `U` undo,
`Enter` save and `Escape` cancel. The editor is an X11 window, not a web view.
Recording requires `ffmpeg` with X11grab and H.264 encoding; no recording
process runs while the feature is unused. X11 captures visible screen pixels;
obscured windows are not independently redirected. The tool reads the X11
display only: there is no Wayland capture path, and under the preliminary
Wayland session it is limited to what XWayland sees and is unvalidated. Text
annotations, OCR, recording audio, advanced video editing and Spectacle/KDE
DBus API compatibility are not currently implemented.

## Configuration locations

- Settings file, resolved in this order (`settings_path` in `crates/flamewm-wm-x11/src/runtime.rs`):
  1. `FLAMEWM_SETTINGS_PATH` (exact file path);
  2. `$FLAMEWM_CONFIG_DIR/settings.toml`;
  3. `$XDG_CONFIG_HOME/flamewm/settings.toml`, or `~/.config/flamewm/settings.toml` when `XDG_CONFIG_HOME` is unset.
- Wallpapers: `$XDG_DATA_HOME/flamewm/wallpapers` (default `~/.local/share/flamewm/wallpapers`), per `crates/flamewm-integrations-linux/src/media_store.rs`.
- Installed read-only assets: `/usr/share/flamewm/` (see `packaging/install-manifest.txt`).

## Build and development

Ubuntu/Debian development dependencies:

```bash
./scripts/install-deps-ubuntu.sh
```

Then, from the repository root:

```bash
./scripts/doctor        # environment and library checks
./scripts/check         # build and native checks
./scripts/xephyr        # run FlameWM inside a nested Xephyr display
```

Packaging and release gates:

```bash
./scripts/package       # packaging only; does not run Cargo or tests
./scripts/release-gate  # runs ./scripts/check and ./scripts/package and verifies checksums; does NOT run ./scripts/xephyr or ./scripts/verify-visual
```

The interactive contract is a release build. Debug builds are not accepted as product performance evidence.

Visual comparison (`./scripts/verify-visual`) and the other acceptance scripts under `scripts/` and `tools/` are separate gates and must be run explicitly.

Wayland development (preliminary; no display or hardware needed for the first two):

```bash
./scripts/build-deb                        # release build with --features flamewm-wm-wayland/drm
./scripts/tests/wayland-session-matrix.sh  # wrapper backend/gate matrix, runs without a session
flamewm-wayland --backend=nested           # nested winit window inside your current session
```

The release build preflights the `libinput`, `gbm`, `libdrm` and `libudev` development files plus an `Xwayland` binary on `PATH`; it installs nothing. Details and the unverified list are in [`docs/WAYLAND_PLAN.md`](docs/WAYLAND_PLAN.md).

## Install, upgrade, and uninstall

`./scripts/package` writes the Linux archive, the source archive, and, when `dpkg-deb` is available, the `.deb` under `.target/dist/` (file names follow the workspace version printed by `scripts/version`; for example `flamewm_<version>_amd64.deb`). See [`docs/INSTALL.md`](docs/INSTALL.md) and [`docs/PACKAGING.md`](docs/PACKAGING.md).

**Neither installed session has been accepted end to end in this revision.** Install, upgrade and uninstall are documented from `scripts/package` and `packaging/debian/control.in`. The X11 session is the documented default; the Wayland session has no successful hardware login on record (see [Wayland current limitations](#wayland-current-limitations)).

Install:

```bash
sudo dpkg -i .target/dist/flamewm_<version>_amd64.deb
```

Upgrade: install a newer `flamewm_*_amd64.deb` with the same `dpkg -i` command.

Uninstall:

```bash
sudo dpkg -r flamewm
```

Without `dpkg` (not tested), the Linux archive can be unpacked as root. Its entries are prefixed `flamewm-<version>/`, so strip that component:

```bash
tar -xzf .target/dist/flamewm-<version>-linux-x86_64.tar.gz --strip-components=1 -C /
```

## Memory

Measured on a private Xephyr display. Five paired release runs each of an older build and the current build:

- Combined WM + Shell idle RSS: median about 30.7 MB (older build) and 31.1 MB (current build); maximum 31.23 MB.
- All 10 runs were under the 34 MiB (35,651,584 B) target.
- PSS about 14.7 MB.
- The current build is not measurably smaller than the older build.
- Not included: the desktop process (about 44 MB RSS) and X server memory.
- Not measured: real Xorg, frame p95, and full-session numbers.

## Crate architecture

Crate roles are taken from the workspace `Cargo.toml` manifests. Layering rules are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md):

| Crate | Role |
| --- | --- |
| `crates/flamewm-wm` | Binary `flamewm`; X11 window manager authority, built on `flamewm-wm-x11` |
| `crates/flamewm-wm-x11` | X11 window manager library (`x11rb`): client discovery, frames, focus, workspaces, snap geometry |
| `crates/flamewm-wm-wayland` | Binary `flamewm-wayland`; preliminary Smithay-based Wayland compositor (DRM/KMS and nested winit backends) |
| `crates/flamewm-platform` | Product services and policy (settings, shortcuts) |
| `crates/flamewm-*-core` | Pure product models (for example `flamewm-settings-core`, `flamewm-shell-core`) |
| `crates/flamewm-shell` | V8 shell controller: panel, Start, popovers |
| `crates/flamewm-settings` | Settings application |
| `crates/flamewm-lock` | Lock screen core (PAM, settings, lock state) |
| `crates/flamewm-render-compiler` | Build-time HTML/CSS compiler to RWRB/2 |
| `crates/flamewm-render-x11` | Xlib/Xft native renderer and FFI boundary |
| `crates/flamewm-reactor` | Native event sources |
| `crates/flamewm-x11-canary` | X11 acceptance scenario binary |

The `.vendor/` directory holds historical reference corpora only. No active crate may include, link, or load `.vendor/**`.

## Licensing

- The workspace package license in [`Cargo.toml`](Cargo.toml) (`workspace.package.license`) is `LGPL-2.0-only`; the full text is in [`LICENSE`](LICENSE). [`packaging/debian/copyright`](packaging/debian/copyright) also declares LGPL-2.0-only.
- **Known mismatch:** `crates/flamewm-render-compiler/Cargo.toml` declares `license = "MIT"` and [`LICENSES/RustWebRender-MIT.txt`](LICENSES/RustWebRender-MIT.txt) contains MIT text, while the rest of the workspace is LGPL-2.0-only. The license for that crate is pending maintainer confirmation; this README does not resolve it.

## Documentation

- [`CHANGELOG.md`](CHANGELOG.md): release history.
- [`docs/INSTALL.md`](docs/INSTALL.md), [`docs/PACKAGING.md`](docs/PACKAGING.md): install and packaging.
- [`docs/WAYLAND_PLAN.md`](docs/WAYLAND_PLAN.md): Wayland session implementation, diagnostics and the unverified list.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): product boundary and ownership.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md): performance contract and targets.
- [`CONTRIBUTING.md`](CONTRIBUTING.md), [`SECURITY.md`](SECURITY.md).
