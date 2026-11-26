# Unit u63-idle-desktop
DONE
Area: Idle desktop composition: every periodic wakeup in desktop+shell+reactor (clock, tray, weather, usage); batch to one deadline queue
Owned paths: crates/flamewm-desktop, flamewm-shell, flamewm-reactor
Task: Batch nine. Read .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest hotspot by composition share in normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u63-idle-desktop.md.
