# Unit p88-panic-abort-decision
STATUS: OPEN

Cargo.toml release has panic=abort, so diagnostics::guard (catch_unwind) is a no-op in production. Audit: (1) rg every guard( call site and list what each is meant to protect; (2) read the session supervisor (crates/flamewm-wm/src/bin/flamewm-session) and list which child crashes are respawned today and with what backoff; (3) write docs/PANIC_STRATEGY.md with the facts, the options (keep abort + process respawn; unwind only for specific bins via a profile; unwind everywhere) and the exact measurement to take (binary size and idle RSS with panic=unwind for shell and desktop). No Cargo.toml edits.
