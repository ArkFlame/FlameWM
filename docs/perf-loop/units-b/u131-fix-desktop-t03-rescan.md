# Unit u131-fix-desktop-t03-rescan
DONE
Area: Reliability. flamewm-desktop test entry::workspace_sync::tests::t03_workspace_visual_dirty_does_not_rescan_or_refresh_launchers fails at crates/flamewm-desktop/src/workspace_sync.rs:121: assertion `!projection.contains("rescan")`.
Owned paths: crates/flamewm-desktop/src/workspace_sync.rs, crates/flamewm-desktop/src/main.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep). Run `cargo test -p flamewm-desktop t03_workspace` once. Use git log -p to see which slice made a workspace-visual-dirty projection trigger a launcher rescan, and restore the intended behaviour (visual dirty must not rescan/refresh launchers) in the product code; do not weaken the test. Report root cause and exit code.
