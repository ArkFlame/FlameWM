# Unit u136-explorer-thumb-cache-caps
DONE
Area: Explorer: thumbnail and icon caches
Owned paths: crates/flamewm-explorer/src/thumbs.rs, crates/flamewm-explorer/src/thumbs, crates/flamewm-explorer/src/appicon.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Verify the thumbnail and app-icon caches are LRU with a byte cap (not just an entry count), decode off the UI thread, cancel decodes for rows scrolled out of view, and are dropped when the window is hidden for a while. Fix whichever of those is missing with the smallest change. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
