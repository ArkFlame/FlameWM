# u47 terminal failure paths (orchestrator audit, no code change)
Workers were unavailable (opencode-go and openai providers returned 401 auth errors), so the orchestrator audited directly.
| file:line | path | finding |
|---|---|---|
| terminal/src/pty.rs:53 | $SHELL missing | falls back to /bin/sh already |
| terminal/src/session.rs:68, pty.rs:95 | thread spawn | results handled; unwrap/expect in this crate are all in #[cfg(test)] code |
| render-core glyphs.rs | missing font | bundled DejaVuSansMono last resort (earlier unit) |
PTY open failure: pty.rs spawn returns io::Result (every step uses ?), app.rs start_pane maps it to Err(String); no panic path. Remaining: a fault-injection test that forces a bad program (needs a build slot). PASS for panics, PARTIAL for test coverage.
