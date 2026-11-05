# FlameWM Domain Map

One page for agents: which crate owns what. Authority rules live in
[`ARCHITECTURE.md`](ARCHITECTURE.md); exact path exceptions live in
[`ARCHITECTURE_EXCEPTIONS.toml`](ARCHITECTURE_EXCEPTIONS.toml). Verify against source before relying on a row.

## Rules

- One owner per mechanism. Product code goes through typed facades.
- X11 WM authority: `flamewm-wm-x11` (`x11rb`). Wayland compositor authority: `flamewm-wm-wayland`.
- UI crates (shell, desktop, apps) issue commands and render; they are never WM authority.
- `.vendor/**` is read-only reference. No active crate may depend on or load it.

## Session and window-manager authority

| Crate | Responsibility | Owns |
| --- | --- | --- |
| `flamewm-wm-x11` | X11 window manager | SubstructureRedirect, reparenting, ICCCM/EWMH state, focus, grabs, move/resize, workspace state, work-area geometry |
| `flamewm-wm` | X11 WM binary/session wiring | Process entry, session bootstrap |
| `flamewm-wm-wayland` | Wayland compositor (Smithay) | Window model, xdg/layer-shell, seat, DRM/libinput (feature `drm`), frame scheduling, XWayland |
| `flamewm-x11-canary` | X11 regression harness | Scenario tests against a live WM (test tool, not product) |
| `flamewm-window-core` | Pure window model helpers | Window-state data without X11/Wayland types |
| `flamewm-session-core` | Session policy model | Session state transitions |
| `flamewm-x11-desktop` | X11 desktop glue | Desktop-session wiring for the X11 path |

## Shared product policy (consumed by both sessions)

| Crate | Responsibility | Owns |
| --- | --- | --- |
| `flamewm-api` | Public product contracts | Workspace grid (`workspace`), shortcut action names, system/graphics types |
| `flamewm-platform` | Product services and policy | Shortcut registry, terminal/open-files actions, settings services, workspace facade |
| `flamewm-reactor` | Native event sources | The single owner of native event-loop sources |
| `flamewm-dbus-reactor` | D-Bus event source | Bus watches (`BusWatch`) |
| `flamewm-debug` / `flamewm-profiler` | Diagnostics | Debug logging, profiling probes |

## Pure models (no I/O, no UI)

| Crate | Responsibility |
| --- | --- |
| `flamewm-desktop-core` | Desktop model: file actions, file-manager policy, sticky state, drag state |
| `flamewm-shell-core` | Shell presentation models (e.g. workspace pager cells, labels) |
| `flamewm-settings-core` | Settings schema and model |
| `flamewm-control-core` | Control-surface command model |
| `flamewm-control-wire` | Control wire format |
| `flamewm-assistant-core` | ARK AI chat client, history store, model downloads |
| `flamewm-downloads-core` | Download tracking model |
| `flamewm-image-core` | Image decode/model helpers |
| `flamewm-ui-core` | UI model shared by UI crates |
| `flamewm-ribbons` | Ribbons screensaver model and CPU rasterizer |

## Integrations (Linux services)

| Crate | Responsibility | Notes |
| --- | --- | --- |
| `flamewm-integrations-linux` | Linux desktop services: icons and icon themes, audio, backlight, mounts, MPRIS, NetworkManager, notifications, portal file chooser, power, PulseAudio, themes | Icon resolution owner (`icons/`, `icon_theme`, `icon_service`) |
| `flamewm-integrations-core` | Integration-neutral types | |
| `flamewm-applications` | Desktop-entry discovery and launch feedback | Own X connection for startup notification (exception listed) |
| `flamewm-audio-helper` | PulseAudio bridge process | Spawned lazily |
| `flamewm-clipboard` | CLIPBOARD selection owner | Own x11rb connection (exception listed) |
| `flamewm-remote-fs` | sftp/fish/ssh client over OpenSSH | |
| `flamewm-xsettings` | XSETTINGS owner | |
| `flamewm-lock` | Lock screen core (PAM, lock state) | |

## Rendering

| Crate | Responsibility |
| --- | --- |
| `flamewm-render-compiler` | Build-time HTML/CSS compiler for FlameWM Render |
| `flamewm-render-core` | Dependency-free precompiled runtime: layout, text, measurement |
| `flamewm-render-x11` | Minimal Xlib/Xft backend (the FFI boundary; unsafe allowed here) |
| `flamewm-ui-document` | Backend-neutral precompiled UI document runtime |
| `flamewm-ui` / `flamewm-ui-x11` | Typed UI bridge to the X11 renderer |
| `flamewm-skin` | Renderer-neutral visual tokens and recipes |

## Product applications (UI, X11 clients)

| Crate | Responsibility |
| --- | --- |
| `flamewm-shell` | Desktop panel, popovers, settings presentation (V8 shell controller) |
| `flamewm-desktop` | Desktop surface: wallpaper, icons, projection of desktop-core state |
| `flamewm-settings` | Settings application |
| `flamewm-explorer` | File explorer |
| `flamewm-finder` | File finder (filename index) |
| `flamewm-archiver` | Archive core (zip/tar/xz/bz2) |
| `flamewm-archiver-app` | Archive manager UI |
| `flamewm-assistant` | Assistant chat window |
| `flamewm-imageviewer` | Image viewer |
| `flamewm-terminal` | Terminal emulator |
| `flamewm-task-manager` | Process monitor |
| `flamewm-screenshot` | Screenshot tool |
| `flamewm-apps` | Multi-call binary: one copy of shared code, hard links per app |

## Descriptions not verified

Crates without a `description` in `Cargo.toml` (`flamewm-debug`, `flamewm-profiler`, `flamewm-session-core`,
`flamewm-window-core`, `flamewm-x11-desktop`, `flamewm-xsettings`, `flamewm-downloads-core`, `flamewm-ui`,
`flamewm-ui-core`, `flamewm-settings-core`, `flamewm-control-*`, `flamewm-clipboard`) are described from
their names and direct use only. Re-verify before relying on them.
