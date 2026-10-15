![FlameWM](assets/branding/flamewm-wordmark.png)

# FlameWM Rust WebRender 0.0.9

FlameWM is a clean-room, X11-only window manager and desktop shell written in Rust. This repository is the Rust WebRender branch: the shell UI is authored as HTML/CSS and compiled at build time into native Rust rendering, with no browser runtime.

FlameWM 0.0.9 supports **X11 sessions only**. Wayland is not supported.

Version and release history: [`FLAMEWM_VERSION`](FLAMEWM_VERSION), [`Cargo.toml`](Cargo.toml) (`workspace.package.version`), and [`CHANGELOG.md`](CHANGELOG.md).

## Status

- Source-level 0.0.9 release. Remaining product surfaces are tracked in [`docs/TODO.md`](docs/TODO.md).
- FlameWM does not claim parity with the historical IceWM or FlameWM C implementations.
- Benchmarks are **pending**. No performance numbers are published here. Measured comparisons are produced with [`tools/flamewm_perf_compare.py`](tools/flamewm_perf_compare.py) and will be recorded once run on target hardware. The memory target is described in [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

## Features

- X11 window manager with reparenting, focus and activation, minimize/maximize/fullscreen, interactive move and resize, and snap previews (`crates/flamewm-wm`, `crates/flamewm-wm-x11`).
- Virtual workspaces with keyboard switching.
- Desktop with icons, context menus, and wallpaper loading (`crates/flamewm-desktop`, `crates/flamewm-integrations-linux`).
- Panel, Start menu, and status popovers rendered by the shell (`crates/flamewm-shell`).
- Settings application with typed settings transactions across the platform services (`crates/flamewm-settings`, `crates/flamewm-platform`).
- Breeze-styled lock screen with PAM authentication (`crates/flamewm-lock`, `packaging/pam.d/flamewm-lock`).
- Volume on-screen display driven by the XF86 audio keys.

## X11 limitations

- X11 session only. There is no Wayland or XWayland-independent compositor path.
- Requires X11, Xft, and Fontconfig runtime libraries.
- The renderer crate (`crates/flamewm-render-x11`) is the explicit Xlib/Xft FFI boundary; the window manager itself uses safe `x11rb` protocol APIs.
- FlameWM does not replace a display manager. Select it from the session chooser after installation.

## Default shortcuts

The defaults below come from `crates/flamewm-platform/src/shortcuts.rs`. Bindings are configurable through settings.

| Action | Default |
| --- | --- |
| Toggle Start menu | `Super` (left Super, bare tap) |
| Switch workspace left / right | `Ctrl+Super+Left` / `Ctrl+Super+Right` |
| Switch workspace up / down | `Ctrl+Super+Up` / `Ctrl+Super+Down` |
| Close window | `Alt+F4` |
| Lock session | `Super+L` |
| Volume up / down / mute | `XF86AudioRaiseVolume` / `XF86AudioLowerVolume` / `XF86AudioMute` |
| Minimize / maximize window | unassigned |

## Configuration locations

- Settings file: `$XDG_CONFIG_HOME/flamewm/settings.toml` (default `~/.config/flamewm/settings.toml`), per `crates/flamewm-wm-x11/src/runtime.rs:892-908`. `FLAMEWM_CONFIG_DIR` takes precedence over XDG config; `FLAMEWM_SETTINGS_PATH` takes precedence over both.
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

## Install, upgrade, and uninstall

These steps are documented but **not tested** in this README revision.

Build release binaries, then package them with `./scripts/package` (see [`docs/INSTALL.md`](docs/INSTALL.md) and [`docs/PACKAGING.md`](docs/PACKAGING.md)). When `dpkg-deb` is available, the package is written to `.target/dist/flamewm_0.0.9_amd64.deb`.

Install:

```bash
sudo dpkg -i .target/dist/flamewm_0.0.9_amd64.deb
```

Upgrade: install a newer `flamewm_*_amd64.deb` with the same `dpkg -i` command.

Uninstall:

```bash
sudo dpkg -r flamewm
```

Without `dpkg` (not tested), the Linux archive `flamewm-0.0.9-linux-x86_64.tar.gz` can be unpacked as root. Its entries are prefixed `flamewm-0.0.9/`, so strip that component:

```bash
tar -xzf flamewm-0.0.9-linux-x86_64.tar.gz --strip-components=1 -C /
```

## Crate architecture

Crate roles are taken from the workspace `Cargo.toml` manifests. Layering rules are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md):

| Crate | Role |
| --- | --- |
| `crates/flamewm-wm` | Binary `flamewm`; X11 window manager authority, built on `flamewm-wm-x11` |
| `crates/flamewm-wm-x11` | X11 window manager library (`x11rb`): client discovery, frames, focus, workspaces, snap geometry |
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

- The workspace metadata in [`Cargo.toml`](Cargo.toml) and [`packaging/debian/copyright`](packaging/debian/copyright) declare `LGPL-2.0-only`; the full text is in [`LICENSE`](LICENSE).
- `crates/flamewm-render-compiler/Cargo.toml` declares `MIT`, and [`LICENSES/RustWebRender-MIT.txt`](LICENSES/RustWebRender-MIT.txt) contains MIT text. The license for that crate is pending maintainer confirmation.
- Third-party notices and icon/theme licenses are in [`LICENSES/`](LICENSES/).

## Documentation

- [`CHANGELOG.md`](CHANGELOG.md): release history.
- [`docs/INSTALL.md`](docs/INSTALL.md), [`docs/PACKAGING.md`](docs/PACKAGING.md): install and packaging.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): product boundary and ownership.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md): performance targets.
- [`CONTRIBUTING.md`](CONTRIBUTING.md), [`SECURITY.md`](SECURITY.md).
