# Unit p30-idle-wakeups-x11
STATUS: PARTIAL

Crates flamewm-wm-x11 and flamewm-x11-desktop: same audit as p29 for X11 (timers, polling, redundant redraws when idle). Event driven or coalesced; visuals identical.
## Result
STARTED
- (crates/flamewm-wm-x11/src/idle.rs:104, one one-shot MIT-SCREEN-SAVER timer armed only while lockTimeoutMinutes is nonzero, deadline clamped 1s..30s, disabled on query error and removed via stop_timer at teardown; no idle polling, change NONE, risk none) UNBUILT
- (crates/flamewm-wm-x11/src/wm/event_loop.rs:96, reactor.dispatch(wait) blocks with wait None unless a real deadline from transitions, deferred chrome, hot corner, drag edge, ping, blue light, MPRIS retry or display revert is pending; X fd wakes dispatch; change NONE, risk none) UNBUILT
- (crates/flamewm-wm-x11/src/event_pump.rs:48, contiguous same event+child MotionNotify collapsed to the latest event so no redundant per-motion redraw; change NONE, risk none) UNBUILT
- (crates/flamewm-wm-x11/src/runtime/mod.rs:525, battery CPU 1s sampling armed only while the battery menu is open and wake_deadline rewritten only when the deadline moves; change NONE, risk none) UNBUILT
- (crates/flamewm-wm-x11/src/outline_tune.rs:125 and wm/paint.rs:310, outline tune and titlebar backdrop refresh are gated by env var / one-shot readiness flag, no per-turn file read or repaint while idle; change NONE, risk none) UNBUILT
- (crates/flamewm-x11-desktop/src/lock.rs:80, lock supervisor blocks on child.wait and sleeps only after an actual crash; all other sleeps are cfg(test); change NONE, risk none) UNBUILT
- (crates/flamewm-x11-desktop/src/ports.rs:243, add_poll/add_timer/defer return unsupported, so this crate owns no polling or timer sources; change NONE, risk none) UNBUILT
- (production edits NONE across both crates; no tests edited; no .vendor reference consulted as a runtime input; .vendor dirs listed only, precedent not needed because local source already shows the event-driven pattern)

PARTIAL audit complete: no production edits were made because every timer, poll and redraw path in flamewm-wm-x11 and flamewm-x11-desktop is already event-driven or deadline-coalesced, and no unconditional idle wakeup or redundant idle redraw was found; visuals are untouched. No build, test or runtime measurement was run per brief, so all entries are static-evidence only and UNBUILT. Skills loaded: flamewm-source-first, flamewm-x11-window-manager, flamewm-performance-proof, scoped-perf-tests, flamewm-rust-systems, no-adhoc-scripts, lean-code.
