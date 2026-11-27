# Unit p66-desktop-idle-steady
STATUS: OPEN

Crate flamewm-desktop: steady-state idle wakeups (measured ~9/s per 4 s sample, hotlist B1): find every timer/poll (rg 'Duration|timerfd|sleep|poll|tick'), coalesce or make event driven. Output identical. Max 3 edits.

## Result
STARTED
