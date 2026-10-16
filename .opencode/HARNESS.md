# FlameWM Harness

Before substantive work: read `AGENTS.md`, the workspace version (`scripts/version`), this file, and
`ROUTES.md`; inspect current source and Git state; select/load route skills.
Source and executed runtime evidence outrank docs, plans, and references.

- Use active FlameWM owners. `.vendor/**` is read-only evidence, never active dependency or runtime input.
- Keep task evidence under ignored `.agents/runtime/`, scoped by job. Record route, skills, commands, results, paths, and blockers.
- One Cargo owner at a time. Set `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2` for Cargo work.
- Repair from exact diagnostics. If a repair fails, record competing hypotheses and escalate with discriminating evidence; do not guess or loop.
- Runtime claims require direct fresh runtime evidence. State unrun checks and limits.

## 1.3 Compact-result / context-budget contract

- Worker result capsules are <=80 lines / <=1200 tokens: status lines, changed paths, commands/results, risks, blockers.
- Full evidence lives under `.agents/runtime/<cohort>/<job>/`; capsules link to it, never inline it.
- Dependents consume only frozen contract capsules from the coordinator; they re-read source, never prior worker full logs.
