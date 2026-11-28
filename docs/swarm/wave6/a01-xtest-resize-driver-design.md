# a01 XTEST resize driver — design

Status: UNBUILT (no `cargo` run by this worker; the orchestrator builds and verifies).

## Scope

A standalone binary crate `tools/x11-resize-drive` and a bounded check `scripts/x11-resize-check`.
No product/WM code changed; visuals stay pixel-identical.

## Why standalone

Root `Cargo.toml:3` declares `members = ["crates/*"]`. That glob does not include `tools/`, so a
crate at `tools/x11-resize-drive` is not a workspace member. Its own `Cargo.toml` therefore declares
an empty `[workspace]` root and copies the workspace's pinned `x11rb = "=0.14.0"` with
`default-features = false, features = ["xtest"]`. No path dependencies and no new dependencies.

## Reference-first evidence

- `.vendor/xfwm4-master/src/events.c` (button dispatch around line 4296) and
  `.vendor/xfwm4-master/src/moveresize.c`: a button press on a corner window starts a resize and the
  drag is recomputed from the newest root pointer coordinates.
- `.vendor/xfwm4-master/src/frame.c` (corner windows around lines 283, 1063, 1076): eight corner/side
  input windows around the frame.
- xdotool semantics (as recorded in `tools/flamewm_xtest.py`): absolute `fake_motion` + button
  press/release via XTEST, with release attempted even when a later event fails.

**Choice**: port the *semantics*, not the code — press button 1 four pixels inside the frame's
bottom-right grip and emit synthetic absolute pointer motion under a bounded button hold; always
release the button and any held modifiers on every path. No vendor file is linked, loaded, executed,
or copied into the build.

## Active owner evidence

- `crates/flamewm-wm-x11/src/frame/layout.rs:1-39`: eight resize zones, 16 px bottom-right grip,
  `RESIZE_EDGE_RB = 2`. This is why the target is the frame corner, not the client rect corner.
- `crates/flamewm-wm-x11/src/wm/drag_modifier.rs:23-28`: `is_meta_resize_press` requires button 3 plus
  Control plus the configured drag modifier (Super default, Alt when `FLAMEWM_DRAG_MODIFIER=alt`).
  This is why `--mode alt-button3` holds both `Control_L` and `Alt_L` before pressing button 3.
- `crates/flamewm-render-x11/tests/xdnd_two_windows.rs:58-86`: in-repo pattern for interned atoms,
  `query_tree(root)`, and `_NET_WM_NAME`/`WM_NAME` property comparison.

## Behavior

1. Connect to `DISPLAY` (or `--display`), find a root child whose `_NET_WM_NAME` or `WM_NAME` equals
   the positional `WM_NAME` argument.
2. Resolve the reparented frame (else the client) root-relative rectangle; aim 4 px inside its
   bottom-right corner.
3. `corner` mode (default): move there and press button 1. `alt-button3` mode: press `Control_L` and
   `Alt_L`, then press button 3.
4. Emit 600 synthetic absolute motions (`--steps`) spread evenly over 1000 ms (`--duration-ms`),
   with nanosecond deadline pacing (1 kHz-class scheduling granularity). Default displacement is
   `+120,+80`, overridable with `--delta X,Y`.
5. Release button and modifiers on success or motion failure; flush each request with `.check()`.
6. Print exactly one stdout line: `RESIZE-DRIVE steps=<n> elapsed_ms=<ms>`. Errors go to stderr with
   exit status 1.

`scripts/x11-resize-check` starts Xvfb `:99` with `-noreset`, flamewm, and a titled xterm; it gates on
real `xprop` round trips bounded by `timeout`, runs the driver under `timeout`, reaps children with
`wait`, and prints `PASS` only on exit 0 plus the expected report line. It uses no Python and no
`sleep` polling.

## Risks / unverified

- UNBUILT: no compile, clippy, or runtime run by this worker; API signatures were checked against
  the local `x11rb-0.14.0` registry source, not a compiler.
- The exact corner grip hit depends on the frame's reparented geometry and titlebar scale; the 4 px
  inset is chosen to stay inside the 16 px grip and may need tuning on a different skin scale.
- The check script requires `Xvfb`, `xprop`, `xterm`, `timeout`, and prebuilt `flamewm` and driver
  binaries; missing prerequisites produce `FAIL`, not a silent pass.
