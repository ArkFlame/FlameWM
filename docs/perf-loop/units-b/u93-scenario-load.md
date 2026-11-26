# Unit u93-scenario-load
OPEN
Area: Make scripts/compose-scenario produce real load. Findings: it targets nested XWayland (:2, correct) but /usr/bin/flamewm-terminal logs only startup lines and the compositor stays 99.8% wait. Find why (does the client map a window? does the PTY feed run? is the client waiting for a Wayland-only path?), try the native Wayland client path (WAYLAND_DISPLAY only, no DISPLAY) if the terminal supports it, and add a guard: print LOAD-NOT-REACHED and exit 3 when the busiest tick has non-wait share < 5%, so an idle run is never reported as active.
Owned paths: scripts/compose-scenario
Task: Read .agents/skills/no-adhoc-scripts/SKILL.md first. No python/sleep. No cargo/git. Log to docs/perf-loop/LOG-u93-scenario-load.md.
