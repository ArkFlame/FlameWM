# Unit p29-idle-wakeups-wayland
STATUS: OPEN

Crate flamewm-wm-wayland: find timers and polling loops that wake when nothing changed (rg 'Timer|interval|sleep|poll'); make them event driven or coalesced, visuals identical. Report share estimate per source.
