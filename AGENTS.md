# FlameWM Agent Governance

Before substantive work, read the workspace version (`scripts/version`, from `Cargo.toml`), `.opencode/HARNESS.md`, and
`.opencode/ROUTES.md`; load routed skills and record route evidence. Read
`.agents/README.md`, `.agents/CONTRACTS.md`, and `.agents/JOBS.md` for assigned
work.

Current source and runtime evidence outrank documentation, plans, and reference
material. Read `docs/ARCHITECTURE.md` before product-structure changes. One
owner exists per mechanism: product code uses typed facades; `flamewm-reactor`
owns native event sources; UI never becomes WM authority. `.vendor/**` is
read-only reference, never dependency or runtime input. Exceptions are explicit
path entries in `docs/ARCHITECTURE_EXCEPTIONS.toml`; no wildcards.

Keep changes assigned and preserve unrelated work. Runtime, build, and release
claims require fresh executed evidence; static inspection is not proof.
