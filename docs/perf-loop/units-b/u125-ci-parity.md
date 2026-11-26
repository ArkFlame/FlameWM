# Unit u125-ci-parity
OPEN
Area: CI parity: read .github/workflows/*.yml and list every command CI runs (fmt, clippy flags, tests, makepkg); note which of them scripts/perf-loop check does NOT run; propose the cheapest local equivalent and write it to docs/perf-loop/CI-PARITY.md
Owned paths: docs/perf-loop/CI-PARITY.md, .github (read-only)
Task: Batch twenty. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u125-ci-parity.md.
