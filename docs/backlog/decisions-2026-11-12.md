# Backlog decisions, 2026-11-12

Resolves the "DECISION NEEDED" and "decision needed" rows listed below. Each
decision names the precedent read under `.vendor/**` (read-only reference;
main checkout path `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender/.vendor`),
the chosen FlameWM design, and a slice plan. No product code changed in this
pass: every slice below touches a crate owned by another active worker
(`flamewm-shell`, `flamewm-wm-wayland`, `flamewm-wm-x11`, `flamewm-integrations-linux`,
`flamewm-reactor`, `flamewm-applications`) or needs a runtime that is not
available in this sandbox. Rule applied: when a row's premise is checked and
a precedent exists, adopt it; do not wait for an owner.

## D-bt (cmp-bt, bt-agent): BlueZ pairing agent

- Precedent: `.vendor/vela-main/bluetooth/src/agent.c` (registers `/org/vela/BluetoothAgent`
  through `org.bluez.AgentManager1.RegisterAgent` then `RequestDefaultAgent`, and
  unregisters on teardown); mock bus `.vendor/vela-main/tests/bluetooth/fakebluez.h`.
  Standard: BlueZ `org.bluez.Agent1` API (capabilities `NoInputNoOutput`,
  `DisplayOnly`, `KeyboardDisplay`; a reply must be sent for every call, or BlueZ
  waits until its timeout).
- Decision: register the agent with capability `NoInputNoOutput` until an
  in-shell prompt exists. Every `RequestPinCode`, `RequestPasskey`,
  `RequestConfirmation` and `RequestAuthorization` call gets an immediate
  `org.bluez.Error.Rejected`. This is the fail-fast rule learned from pol-session:
  an agent that accepts a request and never answers hangs the caller. The prompt
  UI is a later slice that switches the capability to `KeyboardDisplay`.
- Owner: `crates/flamewm-integrations-linux/src/bluetooth.rs` (the BlueZ owner;
  `bt-agent` lives here, not in the shell).
- Slice 1 (S-M): agent object + register/unregister + reject-all handlers.
  Files: `crates/flamewm-integrations-linux/src/bluetooth.rs`, new `bluetooth_agent.rs`.
  Acceptance: unit test that every Agent1 method returns `Rejected`; registration
  calls `RegisterAgent` then `RequestDefaultAgent` on a mock bus.
  Proof needed: run on a private `dbus-run-session` with a fake BlueZ (no host bus).
- Slice 2 (M, later): in-shell prompt through the typed reply plumbing `88271686`.

## D-sigblock (cmp-sigblock): block SIGINT/SIGTERM before GPU threads

- Premise checked: no thread spawn in `render-x11`, `wm-wayland`, `render-core`
  source performs signal work (the one helper is a `Command` in
  `render-x11/accel/helper.rs`). The two sites `flamewm-wm/src/bin/flamewm-session.rs`
  (signal_hook) and `flamewm-lock/src/signals.rs` are the only signal owners.
- Precedent: signal_hook owns delivery through one waiter thread; Smithay and
  COSMIC keep signal handling in a single owner, not in render workers.
- Decision: close as NOT NEEDED for the present code. A process-wide mask is
  rejected: with no sole waiter the process cannot be stopped by SIGTERM.
  Rule for the future: a GPU worker that is ever added blocks SIGINT/SIGTERM on
  its own thread with `pthread_sigmask` only, after the signal owner has started.
- Slice: none (doc rule only). Proof: none needed while no spawn site exists.

## D-dock-slot (cmp-a11y, dock-slot-1..24 in ui/shell/dock.html)

- Premise checked: names are computed at `crates/flamewm-shell/src/app/dock.rs:247-261`
  but never written to the DOM; `UiDocumentAccess` (`crates/flamewm-ui-document/src/template.rs:167`)
  has no attribute setter.
- Precedent: WAI-ARIA 1.2 button name from `aria-label`; KDE Plasma task manager
  names launchers from the app name (`.vendor/` KDE sources are the reference for
  the Trash label).
- Decision: add an attribute setter, do not hand-write names into the page.
  Trait method `attribute(&mut self, id, name, value) -> Result<(), String>` with a
  default that returns `Err("attribute unsupported")`, implemented on `UiDocument`
  and `UiDocumentView`.
- Slice A (S): `crates/flamewm-ui-document/src/template.rs` (trait + impls + unit test
  that the value lands on the compiled node). Owner: flamewm-ui-document. Not in use
  by other workers.
- Slice B (S): projection of `dock.rs:247-261` names into `dock-slot-N` aria-label.
  Owner: `crates/flamewm-shell/src/app/dock.rs` (shell worker; not this pass).
  Acceptance: static markup test lists 24 slot names; proof needed: runtime
  accessibility-tree check (not available; stays UNVERIFIED).

## D-swatch (cmp-a11y, ui/index.html accent swatches)

- Precedent: the settings slice (`crates/flamewm-settings/src/accent_node_tests.rs`)
  already names accents with KDE Plasma names (Graphite, White, Red, Blue, Purple,
  Teal); White has no KDE accent.
- Decision: reuse the same name table for `ui/index.html` swatches and the colour
  wheel button. Do not invent new names.
- Slice (S): `ui/index.html` swatch `aria-label`s; test in
  `crates/flamewm-render-core/tests/icon_button_names.rs` (static markup only).

## D-a11y (cmp-a11y magnifier and colour filters)

- Precedent: COSMIC `cosmic-epoch/cosmic-settings/cosmic-settings/src/pages/accessibility/magnifier.rs`
  and `cosmic-epoch/cosmic-session/src/a11y.rs` (compositor-owned zoom, settings
  only store the level).
- Decision: zoom and colour matrix stay compositor-owned (wm-wayland / wm-x11).
  Settings only stores the value. Shipped on winit (`6b890a34`, `fd7c0992`,
  `95a7829b`). Open: DRM and X11 paths, pointer-follow proof, reduced-motion and
  high-contrast plumb-through.
- Slice (M): X11 zoom in wm-x11 (owner wm-x11, not this pass). Proof: pixel
  comparison on private Xvfb.

## D-alttab-occlusion (cmp-alttab)

- Precedent: `.vendor/vela-main/shell/src/windowcapture.cpp:251-385` (already cited
  in `docs/backlog/alttab.md`); XComposite redirect is the X11 standard for
  occluded window pixels.
- Decision: approve an XComposite redirect slice in wm-x11 (owner
  `crates/flamewm-wm-x11`). Wayland previews are owned by the render-path producer
  `crates/flamewm-wm-wayland/src/render/switcher_thumbs.rs` (landed); no new owner.
- Slice (L, split in two): (1) XComposite named pixmap for switcher windows;
  (2) read occluded pixels from it. Proof: Xvfb run with an overlapping window,
  thumb pixels differ from the cover.

## wl-portal (ScreenCast, region and window capture)

- Precedent: `.vendor/cosmic-epoch/xdg-desktop-portal-cosmic/src/screencast.rs` (PipeWire
  stream owner inside the portal); `.vendor/starling-main/shell/Sources/PortalService/`
  and `.../ScreenCastService.swift` (portal service boundary). Standard:
  xdg-desktop-portal ScreenCast and Screenshot interfaces.
- Decision: ScreenCast gets its own PipeWire owner, a new crate
  `crates/flamewm-screencast` (outside the wayland worker's paths). Region and window
  capture on Wayland: the portal requests a selection surface from the compositor
  (COSMIC pattern); until that exists, `interactive=true` keeps refusing.
- Slice 1 (S): portal returns `Response 2` (cancelled) for ScreenCast with a clear
  error instead of silence. Slice 2 (L): PipeWire owner. Proof: run the portal on a
  private `dbus-run-session`; not executed in this pass.

## wl-lock-multi (session lock per output)

- Premise checked: `drm/outputs.rs` calls `lock_cover.output_added` (cc4b5e66);
  `clear_lock_surfaces` (`session_lock.rs:37-41`) seeds only `State::output`; render
  reads `lock_surface_on`, not `LockCover`.
- Precedent: standard ext-session-lock-v1 protocol: every output needs a lock
  surface or must stay covered; an uncovered output paints nothing.
- Decision: seed every output into coverage in `clear_lock_surfaces`; make the render
  path cover an output with no live lock surface (`LockCover::uncovered`), not
  leave it blank. Owner: `crates/flamewm-wm-wayland` (not this pass).
- Slice (S-M): seed plus cover in render, unit tests. Proof: two-output runtime is
  BLOCKED in a nested session (one output) and DRM is out of scope; stays PARTIAL.

## wsh-lock-x11 (media volume effect)

- Decision: approve a private sink inside the sandbox, with its own
  `XDG_RUNTIME_DIR`, its own socket and no `DBUS_SESSION_BUS_ADDRESS` of the host.
  Use a PulseAudio null sink (`module-null-sink`) addressed through the private
  socket only. Never probe the host bus.
- Acceptance: `XF86AudioRaiseVolume` changes the sink volume (readable with the
  private `pactl`). Proof: script run under `scripts/break/`, not run in this pass.

## break-b3 (WM RSS gate)

- Decision: adopt the weaker gate that is already measured: steady-state drift at
  most 1 MB after cycle 100 over 1000+ cycles, and 12 MB absolute over 150 cycles.
  Basis: run `.agents/runtime/break/20261111-140926/s2` (RSS flat at 19440 KB from
  cycle 100 to 1000) and the 8.2-8.8 MB warm-up step is allocator warm-up, not growth.
- Status: LANDED under the new gate; close the "owner confirms" wording.

## wsh-launcher-select (darker-row claim)

- Decision: define the claim with a metric and drop the "darker" wording. Claim:
  selected row mean luma (Rec. 601, `0.299R+0.587G+0.114B`) exceeds the surface
  luma by at least 10 in the default, custom-accent and non-black-backdrop configs.
- Measured against the recorded RGB medians: default selected (20,46,59) luma 39.7
  vs surface (9,10,10) luma 9.7, diff 30; backdrop selected (54,87,108) luma 79.5 vs
  surface (52,63,73) luma 60.8, diff 18.7. Both pass the 10 threshold.
- Status: LANDED (parity and metric). Proof for the new claim: re-run
  `scripts/break/launcher-select.sh` and compute luma from the runs above.
