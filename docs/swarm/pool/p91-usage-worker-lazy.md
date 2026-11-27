# Unit p91-usage-worker-lazy
STATUS: OPEN

crates/flamewm-shell/src/usage_host.rs starts a flamewm-usage worker at launch which reads ~/.claude/.credentials.json and ~/.codex/auth.json (seen in strace). Confirm whether the worker starts when the usage tray/popup is disabled or not yet shown. Make the worker start only when a usage provider is enabled in settings AND the popup or tray needs data, and never read credentials otherwise. Keep tray output identical when enabled. At most 2 edits, UNBUILT.
