# AI usage card (tray popup) - design notes

Reference: `.vendor/CodexBar-main/` (read-only). CodexBar's Codex usage payload is
`Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthUsageFetcher.swift`
(`rate_limit.primary_window` / `secondary_window`: `used_percent`, `reset_at`, `limit_window_seconds`).

Precedent choices:
- Pure model and parser live in `crates/flamewm-usage` (no I/O), mirroring `crates/flamewm-weather`.
- Lanes are named by window length (session <= 6h, weekly >= 6d), not by provider label.
- Percent is clamped to 0..=100 for display; raw values are not kept.

Status:
- Slice 1 (model, Codex parser, reset countdown, unit tests): unit-only, `scripts/gate flamewm-usage` EXIT 0.
- Slice 2a (Claude OAuth + Command Code parsers, credential token reader, RFC 3339): unit-only, gate EXIT 0.
- Slice 2b-crate (typed ProviderSnapshot, curl fetch with 10s max-time and secret-free errors, claude_token/commandcode_token from HOME or env, loopback fake-server tests): unit-only, gate EXIT 0.
- Slice 2b-host (flamewm-shell usage_host.rs: 300 s refresh, one worker, enabled = Claude + Command Code): unit-only in gate; settings facade keys NOT added (deferred), config is constants.
- Slice 3 (UI card, tray icon, `scripts/break/usage_card_proof.py`): not started.

Providers: Codex, Claude (OAuth file + API), Command Code (API key env COMMAND_CODE_API_KEY, /alpha) parsed. OpenCode Go: cookie-only in CodexBar (console scrape), not configured. OpenDesign: descriptor only, no usage source, not configured.
