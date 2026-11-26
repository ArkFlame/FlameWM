# u157-nested-drag-driver

Owner files: crates/flamewm-wm-wayland/src (winit backend input path only), scripts/perf-clients

Task: The nested harness cannot drag a window: xdotool, ydotool, wmctrl and wlrctl are not installed, and the compositor has no scripted input. Add an env-gated synthetic drag to the winit backend, for example `FLAMEWM_WAYLAND_SCRIPT_DRAG="x0,y0,x1,y1,steps,ms"`, that after the first dump delay presses at (x0,y0) on a window titlebar, moves the pointer in `steps` equal steps over `ms` milliseconds through the same seat pointer-motion path real input uses, then releases. It must be inert when the variable is unset and must never run in the DRM/tty backend. Then extend scripts/perf-clients (or add scripts/perf-drag) to run the bright.sh + translucent task manager scene, perform the drag, and print backdrop captures per second plus a mid-drag frame dump, so u156's 60/s to about 20/s claim can be measured and the blur lag read from the frame. Add one log counter behind the same env (not always on). Append a docs/perf-loop/LOG.md entry. Run cargo check -p flamewm-wm-wayland once. DONE on its own line only after the check passes and a mid-drag frame was captured.

Rules: keep visuals and features; no python or sleep polling; no git.
