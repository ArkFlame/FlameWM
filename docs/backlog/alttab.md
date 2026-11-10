# Alt+Tab live previews (`cmp-alttab`, competitor gap `cmp-6`)

Status: slice 1 (pure thumbnail policy) landed; capture, wire and card are open. Measured 2026-11-10 on
private Xvfb (`:97`, `-noreset`, 1280x1024x24). No runtime proof of the switcher UI yet.

## 1. Current behaviour (FlameWM)

- Shortcut: Alt+Tab is bound in `crates/flamewm-platform/src/shortcuts.rs:319`; the X11 raw key feed
  classifies it at `crates/flamewm-render-x11/src/app/events/event_loop.rs:300` (`keyboard.alt-tab`).
- WM authority (X11): `crates/flamewm-wm-x11/src/wm/switcher.rs`.
  - `MRU_LIMIT = 64` (`:13`), `Mru::touch` / `order` / `prune` (`:36-44`).
  - `switcher_order` (`:53`): focus history, then never-focused windows in stacking order.
  - `switcher_candidates` (`:193-212`): normal managed windows on the current workspace or sticky.
  - `publish_switcher` builds `ControlSignal::WindowSwitcherChanged` (`:216-231`).
  - `switcher_entry` (`:233-245`) copies **title and app_id only**. No pixels, no size.
- Wire: `crates/flamewm-control-wire/src/signal.rs:3-8` (`SwitcherEntry { title, app_id }`),
  signal at `:45-52`. Decoder at `:261-270`.
- Shell card: `crates/flamewm-shell/src/switcher.rs:1` ("retained card listing every window in MRU
  order"); timer `crates/flamewm-shell/src/app/timers.rs:239` (`tick_switcher`).
- Wayland compositor: `crates/flamewm-wm-wayland/src/control/switcher.rs:66-70` builds the same
  title/app_id entries from `state.model.visible_top_first()`; no capture.
- Existing capture machinery:
  - X11: `crates/flamewm-render-x11/src/backdrop/capture.rs:28` `capture_drawable_rect` (XGetImage on
    any window or pixmap, decoded with live masks). Used for the snap-preview backdrop
    (`app/surface_backdrop.rs:445`) and live backdrop (`app/live_backdrop.rs:397`). Root readback only;
    nothing captures a managed client window for the switcher.
  - Wayland: `crates/flamewm-wm-wayland/src/render/backdrop.rs:1-16`, desktop-layer blurred backdrop,
    stored downscaled. Not per-window.
- Grep for preview/thumbnail in switcher code: none. `docs/backlog/compare.md:14` ("no preview capture
  code found") is correct for the switcher path. `thumbs.rs` in explorer is file thumbnails only.

## 2. What competitors do

- Vela (`.vendor/vela-main`, read-only): Alt-Tab previews are compositor-side Wayland captures.
  - `shell/src/windowcapture.cpp:251-273`: `WindowCapture::capture(identifiers)` starts one
    `ext_foreign_toplevel_image_capture_source` + `vela_capture_begin` job per matching window, only
    when the Alt-Tab UI asks (on open, not resident).
  - `shell/src/windowcapture.cpp:369-385`: finished jobs store a downscaled `m_thumbnails` entry
    (full-size only for the Snipping Tool, flagged `Job::full`, `:37`).
  - `shell/src/windowcapture.cpp:172`: thumbnail removed when the toplevel closes.
  - Vela's capture is lazy and freed on close, and it never keeps per-window buffers at idle.
- Vela feature claim: `docs/competitors/vela/FEATURES.md:22` ("live previews including minimized
  windows; quick Alt+Tab switches without showing the panel").
- Vela's own rejection of resident textures: `docs/competitors/vela/GAP_PLAN.md:62`. Resident
  previews break the idle budget; allowed only if captured lazily on open and freed on close, with a
  transient peak under 2 MiB per preview set. This is the constraint this design follows.
- Others: LordOS rejects hover previews as compositor-owned (`docs/competitors/LORDOS.md:205`); Starling
  has previews only in the Fluent taskbar (`docs/competitors/STARLING.md:123`).

## 3. Measured cost (X11, private Xvfb, 50 samples each)

Command: `DISPLAY=:97 python3 -I` with `libX11` `XGetImage(ZPixmap)` on a mapped, filled window
(harness in `/tmp/claude-1000/capmeasure.py`, deleted with the worktree).

| Source window | ZPixmap bytes | median ms | max ms |
|---|---|---|---|
| 160x100 | 62 KiB | 0.054 | 1.226 |
| 640x400 | 1000 KiB | 2.687 | 12.506 |
| 1280x800 | 4000 KiB | 10.102 | 15.567 |

Reading: capturing the source window and downscaling costs roughly 10 ms per 16:10 window at full size.
A 160x100 read is cheap but only possible if the server already scaled the window. The downscale step
itself and the Wayland path are **not measured**. Client RSS was 20 MiB (not a per-thumbnail figure).
Xvfb does not model obscured windows: XGetImage on an overlapped, non-redirected window returns
whatever is on screen. A correct capture of occluded windows needs `XCompositeRedirectWindow` plus
`NameWindowPixmap` (the `composite` feature is already enabled for `flamewm-wm-x11`), which is
unmeasured.

## 4. Options

| Option | Correct when occluded | Cost at open (8 windows) | Verdict |
|---|---|---|---|
| A. Keep title/app_id only | n/a | 0 | status quo; gap stays open |
| B. X11 XGetImage of the window at open, WM downscales | no (shows overlap) | ~10 ms per full-size window, ~80 ms for 8, plus downscale (unmeasured) | fast to build; wrong pixels when covered |
| C. Composite redirect, server scales to 160x100 (XRender), XGetImage the small pixmap | yes | 0.054 ms per 160x100 read; scaling cost unmeasured; server pixmap per window | recommended X11 path, measure first |
| D. Wayland `ext-image-copy-capture` or offscreen render of the window in the compositor (Vela-style) | yes | unmeasured | recommended Wayland path, after X11 |
| E. Resident per-window textures | yes | per-window memory at idle | rejected: breaks the idle budget (GAP_PLAN.md:62) |

## 5. Recommended design

- The WM owns capture and the pixels. `flamewm-wm-*` decides which windows to capture, when, and at what
  size. The shell only paints what the WM publishes. UI never becomes authority: a missing or late
  thumbnail must not block selection or commit (the existing comment at `wm/switcher.rs:1-6` already
  says the switch works without the shell).
- Lazy: capture on switcher open for the candidate list only. Free on close, on window unmap/destroy,
  and on eviction. No idle timer, no resident buffers.
- Bounded: `ThumbCache` in `switcher_thumbs.rs` (slice 1) caps residency at `MAX_THUMBS = 24` entries of
  160x100x4 (1.46 MiB), under the 2 MiB peak from GAP_PLAN.md.
- Size policy: aspect-fit into 160x100, never upscale (`thumb_size`).
- Cache key: window + source size, so a resize misses instead of showing stale pixels.
- Transport to the shell: pixels must not ride `WindowSwitcherChanged` as a plain array on every
  selection change (that would resend data on each Tab press). Preferred: capture once on open, send one
  shared-memory handle or a per-open id, and let the shell fetch by id. Decided in slice 2, with its own
  protocol test in `flamewm-control-wire`.
- Shell card: plain text, no animation, no `text-shadow` other than the plain shadow already in the
  card, transparency kept (the card's translucent background is optimised, not disabled), and a
  fallback to the current title/app_id row when no thumbnail exists.

## 6. Slice list

1. (landed, this branch) `crates/flamewm-wm-x11/src/wm/switcher_thumbs.rs`: `thumb_size`, `ThumbKey`,
   `ThumbCache` (LRU, bounded, `remove_window`), 8 unit tests. Not yet consumed (`#[allow(dead_code)]`
   on the `mod` line in `wm/mod.rs`).
2. X11 capture on open: option C if the composite path measures under the B baseline, else B. Captured
   into `ThumbCache`, freed on close. Needs a runtime test under Xvfb with a window overlapped by another.
3. Wire: thumbnail handle per entry or per open, protocol test in `flamewm-control-wire` and
   `flamewm-control-dbus` round trip (`signal.rs`, `dbus lib.rs:755`).
4. Shell card: paint thumbnails with the fallback row; check light and dark skins; screenshot evidence.
5. Wayland: option D capture from `flamewm-wm-wayland`, reusing the per-window cache policy.
6. Evidence: `FLAMEWM_TOOLCHAIN_BIN` gate per crate plus a live Xvfb switcher run with at least three
   windows, one occluded, recording capture ms and RSS delta.

Open unknowns: scale cost on the X11 path, composite pixmap server memory, shell pixel transport.
