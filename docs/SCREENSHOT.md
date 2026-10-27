# FlameWM Screenshot — implementation and verification

This is a native Rust executable (`flamewm-screenshot`), run on demand.
It is not a wrapper around Spectacle or a Qt/KDE dependency. Its still-image
capture uses the existing X11 protocol stack (`x11rb 0.14`) and the already
locked pure-Rust PNG encoder (`png 0.18.1`).

## Native capabilities

| Feature | Implemented path | Limit |
| --- | --- | --- |
| All monitors | root-window `GetImage` | X11 only |
| Current monitor | RandR CRTCs + pointer | disconnected outputs excluded |
| Active window | EWMH `_NET_ACTIVE_WINDOW` + frame parent | visible pixels only |
| Window under cursor | `QueryPointer` + frame parent | visible pixels only |
| Drag-region selection | frozen X11 pixmap, XOR outline, Esc cancel | no resizing handles |
| Capture delay | bounded, on-demand | 0–30 sec |
| Cursor in image | XFixes cursor image blending | optional `--cursor` |
| PNG export | native Rust PNG encoder | no JPEG/WebP export |
| Save directory | XDG Pictures/Screenshots; unique filename | creates files with mode 0600 |
| Image clipboard | `CLIPBOARD` `image/png`, `TARGETS`, `INCR` | no KDE clipboard history |
| Native annotations | pen, line, rectangle, ellipse, arrow, blackout, color, undo | no typed text/OCR/blur |
| Screen recording | optional on-demand `ffmpeg -f x11grab` | H.264 video only; no audio |
| Global shortcuts | FlameWM shortcut registry and WM dispatch | configurable in Settings |
| Launcher | XDG desktop entry with capture actions | runs only when invoked |

## Architecture and mutation scope

- `crates/flamewm-screenshot/` owns capture, annotation, clipboard, export and recording orchestration. Neither shell nor WM reimplements pixel processing.
- `flamewm-api` declares screenshot intents; `flamewm-platform` provides default hotkeys and conflict detection.
- `flamewm-wm-x11` only spawns the on-demand command. WM event dispatch must never block for selection or image compression.
- `flamewm-settings` and `ui/settings.html` show the same configurable actions; existing settings transactions remain authoritative.
- Packaging installs one binary and the XDG launcher. `ffmpeg` is an optional suggested package, not part of FlameWM's permanently resident stack.

## User-facing behavior

| Shortcut | Command |
| --- | --- |
| `Print` | `flamewm-screenshot --full --copy` |
| `Shift+Print` | `flamewm-screenshot --region --annotate --copy` |
| `Ctrl+Print` | `flamewm-screenshot --active --copy` |

The annotation editor uses `P` freehand, `L` line, `R` rectangle,
`E` ellipse, `A` arrow, `X` blackout, `C` color, `U` or Backspace undo,
Enter commit and Escape cancel. The topmost 36px in-editor instruction strip is
not drawable. Screen pixels, not that strip, are saved to the PNG.

## Exact checks for an environment with Rust installed

```bash
cargo check -p flamewm-screenshot --all-targets
cargo test -p flamewm-screenshot
cargo test -p flamewm-platform screenshot_shortcuts_are_three_independent_configurable_actions
cargo test -p flamewm-settings hotkey_row_markup_ids_parse_to_their_capture_action
cargo check --workspace --all-targets
./scripts/check
./scripts/package
```

## Native X11 acceptance still necessary

- Check PNG content and color channels on 16-, 24-, 32-bit X11 visuals, two monitors, and mixed monitor origins.
- Check `Print`, `Shift+Print`, `Ctrl+Print` with a freshly mapped and an existing FlameWM session; verify no stuck pointer/keyboard grabs on Escape and after close.
- Check window clipping, taskbar/window occlusion, focus restoration, screen resize, and RandR hotplug.
- Check annotation tool selection/undo and clipboard paste in other applications, including a PNG large enough to trigger `INCR`.
- Check recording with ffmpeg installed and an output path with spaces; verify video playback.
- Compare end-to-end start latency, memory and responsiveness against the unmodified FlameWM build.

### Release blockers / explicit non-parity

Full Spectacle parity is **not** achieved. The supplied Spectacle source also
contains a dedicated video platform, sophisticated Qt/KImageAnnotator-style
editor and OCR features not reproduced here. No Wayland portal capture,
background recorder control panel, microphone/system-audio recording, typed
text/callout annotation, automatic upload, OCR, KWin composite capture of
obscured windows, or Spectacle DBus compatibility is implemented.

The tool is an X11 client only. In the Wayland session it runs on
XWayland and is not a compositor capture path: it cannot see native Wayland
surfaces, and screenshot behaviour under that session is unvalidated.

No native Rust compilation or screenshot-on-X11 run was possible in the
implementation environment without Cargo/rustc and a running X11 server.
Static/package tests are not substitute evidence for real screenshot behavior.
