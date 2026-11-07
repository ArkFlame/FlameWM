# COSMIC comp vs FlameWM Wayland/X11 review (2026-11-07)

Scope: `.vendor/cosmic-epoch/cosmic-comp/src` compared against `crates/flamewm-wm-wayland/src`, `crates/flamewm-wm-x11/src`, `crates/flamewm-window-core/src`. Read-only. `smithay-master` not read. No runtime evidence gathered; all findings are static (grep and short excerpts).

Method: protocol presence counted with `grep -rl` over each source tree (cosmic / ours). Behavior claims come from the excerpts cited below only.

## Already covered in FlameWM (no action)

- Per-output redraw state machine: `crates/flamewm-wm-wayland/src/state.rs:109-118, 269-326` (queued / dirty / vblank-cleared). Stronger than COSMIC's `schedule_render` fan-out (`backend/kms/mod.rs:718-727`), which has no explicit state machine in the excerpt read.
- Vblank-paced flip: `crates/flamewm-wm-wayland/src/drm/mod.rs:56-57, 692`.
- Damage tracking: `render/mod.rs:347-373` (partial repaint with swapchain age), `state.rs:371`.
- xdg-activation, foreign-toplevel, session-lock present (`protocols/compat.rs`, `session_lock.rs`, `lock_global.rs`).

## Top 12 ranked

Ours = count of matches in our three crates (`src` only). COSMIC = count in cosmic-comp `src`.

| # | Technique | COSMIC path:line | Ours | Expected win | Effort | Risk |
|---|---|---|---|---|---|---|
| 1 | `wp_fractional_scale` + viewporter for HiDPI-crisp Wayland clients | `wayland/handlers/fractional_scale.rs`, `state.rs:992-996` | 0 | Sharp fractional-scale apps, correct logical sizes | M | Low |
| 2 | `wp_presentation` feedback (real vblank timestamps) | `backend/kms/surface/mod.rs:1312-1324` (`take_presentation_feedback`) | 0 | Accurate frame timing for video/games; needed for item 4 | S-M | Low |
| 3 | `zwp_pointer_constraints` + relative pointer | `wayland/handlers/pointer_constraints.rs` (4.8K) | 0 | Games and 3D apps lock/relative-move correctly | M | Med (focus interplay) |
| 4 | Frame-time estimation driving frame-callback pacing | `wayland/handlers/compositor.rs:116-128` (`frame_time_estimation`) | 0 | Callbacks timed to real render cost; less wasted frames | M | Low |
| 5 | Direct scanout via dmabuf feedback | `wayland/handlers/compositor.rs:154-165`, `wayland/handlers/dmabuf.rs:70`, `kms/surface/mod.rs:1465-1470` (`send_dmabuf_feedback`) | 0 | Fullscreen video/game zero-copy; lower latency and GPU load | L | High (driver-specific) |
| 6 | `wp_security_context` gating privileged globals | `wayland/handlers/security_context.rs` (2.0K); `state.rs:643-646, 709` (`client_has_no_security_context`) | 0 | Flatpak/sandboxed clients cannot reach privileged protocols | M | Low |
| 7 | `ext_idle_notify` (Wayland) | `wayland/handlers/idle_notify.rs`; `state.rs:720`; `session_lock.rs:51` (`notify_activity`) | 0 (x11 has `wm/idle.rs`) | swayidle / lock tools work on Wayland | S-M | Low |
| 8 | `ext_foreign_toplevel` overlap notify | `wayland/protocols/overlap_notify.rs`; `state.rs:682` | 0 | Panels/clients learn occlusion; can pause work | M | Low |
| 9 | Screencopy sessions with per-session `OutputDamageTracker` | `wayland/handlers/image_copy_capture/user_data.rs:25`, `mod.rs:126-191` | 0 | Screencast/recorders only redraw changed regions | L | Med |
| 10 | DRM lease for non-desktop outputs | `wayland/handlers/drm_lease.rs` (4.6K); `state.rs` | 0 | VR headsets take over outputs | L | Med |
| 11 | XWayland fractional scale (max(1.0) on scale) | `xwayland.rs:650-660` | unverified (`crates/flamewm-wm-wayland/src/xwayland.rs`, 37.6K, not inspected for scale) | Correct X11 app scale | S | Low |
| 12 | Config hot-reload via cosmic-config watcher | `theme.rs:23-38` (`watch_theme`) | partial (`crates/flamewm-wm-x11/src/live_settings.rs`, 18.2K; no Wayland equivalent found) | Settings apply without restart | S-M | Low |

## Notes

- Items 1, 2, 4 are the cheapest path to visible responsiveness and client compatibility. Item 5 depends on item 2 and on driver testing; do not start it before 2.
- Item 3 has the highest app-compat win for games; verify focus-loss release behavior before merging.
- COSMIC has no tearing-control or content-type hint protocol in `src` (0 matches), so those are not recommended from this source.
- Ours has explicit tests for the redraw state machine (`state.rs:1259-1282, 1404-1406`); keep them when touching the render path.

## Not checked (for the user)

- smithay-master: not read. Items 1, 2, 5, 6 depend on smithay APIs that were not verified against the pinned version.
- Crash recovery, a11y hooks, output config persistence, and input latency handling: not inspected.
- No build, test, or runtime run. Effort and risk ratings are estimates.
