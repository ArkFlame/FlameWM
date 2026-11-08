# Vela: issues, PRs and FlameWM exposure

Source: `gh` against `vela-desktop/vela`, fetched 2026-11-08.
Vela licence: GPL-3.0 (`.vendor/vela-main/LICENSE`). Ideas only; no code copied.
Commit history: [COMMITS.md](COMMITS.md).

## Counts

| Set | Total | Open | Closed/Merged |
|---|---|---|---|
| Issues | 16 | 11 | 5 closed |
| PRs | 211 | 11 | 165 merged, 35 closed |

Issue comment counts are low (0-6). Most problems surface as PRs, not issues.

## Problem categories (PR titles, keyword match, overlapping)

| Category | PRs | Main evidence |
|---|---|---|
| Packaging/CI | 50 | Ubuntu DEB, Fedora, Arch, CI freshness |
| GPU/driver | 29 | NVIDIA, Intel ANV, RADV, Vulkan device loss, SHM readback |
| Audio | 14 | volume stalls on hardware readback, stream crash |
| Themes/UI | 16 | theme files, surface styles, personalization |
| XWayland/games | 13 | Proton fullscreen, native resolutions, Wine backdrops |
| Performance | 11 | volume stalls, benchmarks, snapshot retirement |
| Multi-monitor | 11 | dialog placement, per-output fullscreen stacking, rotation |
| Input | 8 | pointer lock, resize borders, on-screen keyboard and touch docs |
| Crash/hang | 6 | stream crash, resize border, Vulkan teardown |
| HiDPI/fractional | 3 | fractional Qt coords, Wayland fractional damage |
| Bluetooth | 2 | shell and daemon |

Crashes are not a single theme. They show up across audio, GPU and compositor.

## Top recurring themes

1. GPU readback and vblank stalls (NVIDIA, SHM capture). Issues #132, #197; PRs #111, #156, #175, #181.
2. Fullscreen games and XWayland (black screens, native resolution). Issues #55, #163; PRs #53, #77, #78, #158, #164, #167.
3. Multi-monitor placement and stacking. PRs #103, #104, #105, #158, #164.
4. Audio hardware stalls. PRs #175, #181, #184, #209.
5. Settings write failures and stale shown values. Issues #208, #218, #221.
6. Taskbar and tray overflow. Issue #219.

## Issue table

| # | Problem | FlameWM affected? | Evidence | Fix effort | Priority |
|---|---|---|---|---|---|
| 55 (closed) | Proton fullscreen black screen while focused | unknown | no Wine-specific handling found in wm-x11 (not exhaustively grepped) | M | P2 |
| 132 (closed) | NVIDIA SHM readback stalls 180 Hz vblank | no | FlameWM is not a Vulkan compositor; `grep DEVICE_LOST\|try_reserve` in `crates/` returned nothing | n/a | n/a |
| 163 (closed) | Native game resolutions on fractional outputs | partial | fractional scale exists in `crates/flamewm-wm-wayland/src/presentation_scale.rs:1-22`; no XWayland native-resolution code found | M | P2 |
| 197 (open) | Ubuntu 26.10 GPU suites fail where Arch passes | unknown | FlameWM has no Vulkan GPU suites | n/a | n/a |
| 208 (open) | Settings shows values after `vela.conf` write fails | partial | monitor layout is honest: `crates/flamewm-settings/src/app_impl_2.rs:376` reports "applied but not saved"; other preference writes not reviewed | S-M | P1 |
| 218 (open) | Preference write failures inconsistent across Settings | partial | same as 208 | S-M | P1 |
| 221 (open) | Keep shown values when config cannot be saved | partial | same as 208 | S | P1 |
| 219 (open) | Taskbar overflow with tray | unknown | dock files are owned by another agent; excluded | - | excluded |
| 222 (open) | Session keyring follows KDE, hides GNOME keyring | unknown | keyring code in `crates/flamewm-session-core/src/lib.rs` not reviewed | S | P2 |
| 210 (closed) | Custom Start button icon | unknown | start menu in `crates/flamewm-shell-core/src/start_menu.rs`; no custom icon config checked | S | P3 |
| 198 (open) | Locally integrated menus in title bars | no | feature request; not applicable | n/a | n/a |
| 174 (open) | Rename "Vela" | no | naming, not code | n/a | n/a |
| 211, 214 (open) | Dictation and feature feedback | no | feature requests | n/a | n/a |
| 50 (open) | UI looks like Windows | no | design opinion; FlameWM's own UI is its own | n/a | n/a |
| 74 (closed) | Native app family | no | product scope | n/a | n/a |

Note: a closed issue here means the issue was closed on Vela's side. It does not mean FlameWM is safe.

## PR table (ideas worth evaluating)

Only items with a clear win and a plausible match to FlameWM are listed. Others are Vela-specific.

| PR | Idea | FlameWM match | Verdict |
|---|---|---|---|
| #165, #164 | Native Wayland fractional damage aligned with rounding | `presentation_scale.rs` | Port idea if fractional damage is wrong. Not verified. |
| #158 | Per-output fullscreen stacking and visible desktop surfaces | `frame/model.rs:191` has Fullscreen mode | Worth porting idea. Not verified. |
| #105, #104, #103 | Dialogs open on the parent or correct monitor | `wm/outputs.rs`, `wm/controllers.rs` exist | Worth verifying on Xvfb with two outputs. |
| #192, #189, #191 | Pointer stays where an app locked it; resize borders only where visible | `frame/layout.rs:37-160` | Worth porting idea. Not verified. |
| #175, #181 | Volume wheel and knob do not stall on hardware readback | `crates/flamewm-wm-wayland/src/control/media.rs`, `flamewm-audio-helper` | Worth evaluating. File owned by another agent right now. |
| #208/#218/#221 | Report failed config writes; keep shown value in step | `flamewm-settings` | Best candidate for a small fix. |
| #147, #90, #84 | Vulkan device loss, OOM-safe reallocations | none | Not applicable; FlameWM has no Vulkan. |
| #182, #224 | Strip and compress the Ubuntu DEB; Fedora 45 | packaging | Not checked. |

## Top FlameWM-affected items to fix

1. Settings write-failure consistency (issues 208, 218, 221). Start with `crates/flamewm-settings/src/app_impl_2.rs:376` and the preference setters in `crates/flamewm-settings`.
2. XWayland native game resolution on fractional outputs (issue 163). Start with `crates/flamewm-wm-wayland/src/presentation_scale.rs` and `xwayland.rs`.
3. Pointer lock and resize border visibility (PRs 189, 191, 192). Start with `crates/flamewm-wm-x11/src/frame/layout.rs`.

## Verification of the top 3 (static, 2026-11-08 on origin/main)

1. Settings write-failure consistency (208/218/221): NOT AFFECTED. Settings writes go through the WM control client (`crates/flamewm-settings/src/app_impl_2.rs:535-566`). Errors come back as `Result` (`control_error`), and values are read back from the client snapshot, not cached in the UI. The only local file write is monitor layout, which already reports "applied but not saved" (`app_impl_2.rs:376`). No fix.
2. XWayland native resolution on fractional outputs (163): UNVERIFIED. `crates/flamewm-wm-wayland/src/xwayland.rs` has no scale or RandR logic. Whether games see logical or native size needs a runtime repro with a fractional nested output. No fix, because the repro was not run.
3. Pointer lock and resize borders (189/191/192): NOT AFFECTED. Pointer lock holds the position (`crates/flamewm-wm-wayland/src/input/pointer.rs:99-106, 251-257`), with unit tests (`pointer.rs` 5 `#[test]`s, including `confined_keeps_previous_position_outside`). Resize handles are clipped to the frame, with `RESIZE_EDGE_RB` capping depth (`crates/flamewm-wm-x11/src/frame/layout.rs:37-39, 157-160`), with tests at `layout.rs:369-371`. No fix.

Result: no code change was needed. Nothing was committed for the fixes.

## Not verified

- No runtime or Xvfb test was run. Every "unknown" and "partial" row is static evidence only.
- Vela's Qt/C++ code was not read or copied.
