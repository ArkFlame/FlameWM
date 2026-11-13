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
- Slice 2 (host fetch and config): not started.
- Slice 3 (UI card, tray icon, `scripts/break/usage_card_proof.py`): not started.

Providers: Codex parser only. Claude, OpenCode Go, Command Code, OpenDesign: not implemented
(enum entries exist; no parser or source yet).
