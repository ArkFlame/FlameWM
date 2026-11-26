# Unit u71-notifications
OPEN
Area: Notification toast 1 s: layout, shadow/blur, timers; history caps; dbus handling off UI thread
Owned paths: crates/flamewm-shell/src/notification_center.rs, osd.rs
Task: Batch ten. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest share in a one-second scenario of normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u71-notifications.md.
