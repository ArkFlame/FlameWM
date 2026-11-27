# Unit p46-idle-shell-timers
STATUS: OPEN

Crate flamewm-shell: after wave 2 audit again: list remaining periodic timers (clock, usage, weather, media, battery). Each must be aligned/coalesced, stop when its popup/tray item is hidden, zero wakeups when nothing changes. Max 3 edits, rest as rows with estimated wakeups/s.
