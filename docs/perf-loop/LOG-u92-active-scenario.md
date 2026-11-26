# u92-active-scenario log

## What exists now

`scripts/compose-scenario` (bash only, no python, no `sleep` binary) starts the nested winit
compositor with `FLAMEWM_PROFILE_COMPOSE=1` and `FLAMEWM_WAYLAND_AUTOSTART=''`, waits for its
socket and its `XWayland ready on DISPLAY=:N` line, then runs one terminal client on that private
display whose PTY receives 2000-line bursts (`yes | head -n 2000`) until the `SECONDS` clock has
passed one second. It kills both, parses the log for every `FLAMEWM-WAYLAND-COMPOSE` /
`COMPOSITION:` block, picks the tick with the highest non-`wait` sum (so the startup tick cannot
win), prints the shares, and names the largest one.

Two details worth keeping:

- Waiting is done with a FIFO held open read-write plus `read -rt SECS -u 9` (`pause`). A pipe
  (`<> <(:)`) hits EOF immediately and does not sleep: measured 10 × `read -rt 0.05 <> <(:)` in
  0.022 s total. `scripts/perf-compose` uses exactly that idiom for its one-second sample, so its
  window is currently near-zero in this bash; that file is not in this unit's owned paths.
- FlameWM's own apps (the terminal included) are X11 clients (`flamewm-ui-x11`), so the client gets
  `DISPLAY=:N` from the nested compositor's own XWayland, not the host's display. Attaching it to
  the host display produces an empty composition.

## Composition captured here

Run on the current host session (nested winit window, one terminal, 1 s of PTY output):

```
 wmwayland.dispatch.wayland share=0.1
 wmwayland.dispatch.control share=0.0
 wmwayland.dispatch.flush share=0.0
 wait share=99.8
```

In the tick that contains the client's frame, `wmwayland.dispatch.render` carried the largest
self wall (2.9–13.0 ms) of any subsystem, ahead of `xwayland` and `control`. The host session here
starves the nested window of frame callbacks (the terminal window is `IsViewable` on the nested
XWayland display and the client burns CPU, but the compositor records one `render` for the whole
run), so the sampled second stays almost all `wait`. The script warns
(`WARN no subsystem above 0.05%: the client likely never painted`) instead of reporting a
misleading zero-share winner. On a host that presents the nested window the same script reports the
real split.

## Change motivated by it

Largest productive share: `render` (per-frame scene build). Cheapest fix in that path, in
`crates/flamewm-wm-wayland/src/render/mod.rs`:

- `scene_elements_into` cloned the whole `Managed` record for every visible window on every painted
  frame (`state.model.get(id).cloned()`). It now borrows it; nothing in the block mutates the model.
- `render_into` built the scene into a fresh `Vec` and then copied every element into the cursor
  list (`elements.extend(build_elements(..))`). `build_elements_into` appends straight into the
  frame's buffer, so the desktop path has one element allocation per frame instead of two plus a
  full move of the element list.

Expected shift: fewer allocations and copies and no per-window struct copy inside `render`; the
scene build and raster shares are unchanged, the alloc/copy part of `render` shrinks. No pixel or
feature change.
