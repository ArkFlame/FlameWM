---
name: flamewm-source-first
description: Read authoritative local source, contracts, and current call paths before proposing or changing FlameWM behavior.
---

## Required sequence

1. Read repository `AGENTS.md`, the workspace version (`scripts/version`), `.opencode/HARNESS.md`, `.opencode/ROUTES.md`, and applicable `.agents` contract and handoff.
2. Inspect Git state before claiming paths or proposing edits. Preserve concurrent and unrelated changes.
3. Locate canonical owner, public contract, callers, tests, configuration, and runtime entry point with repository search. Read complete relevant functions, not only matching lines.
4. Treat current source and fresh command output as authority. Plans, comments, historical code, and external advice are supporting evidence only.
5. State evidence paths and unresolved facts before changing behavior. Route unclear ownership or product boundaries to `flamewm-semantic-modularization`.

## Decision rules

- Follow data, event, and ownership flow end to end before fixing a symptom.
- Verify feature flags, platform guards, error paths, teardown, and call sites before changing a contract.
- Do not infer API behavior, event order, X11 semantics, or build commands from names.
- Use `.vendor/**` only through `flamewm-reference-mining`; it is never active source or runtime input.
- Report changed paths, evidence consulted, commands run, unrun checks, and blockers.
