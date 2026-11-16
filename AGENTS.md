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

## Reference-first rule (`.vendor`)

When in doubt about how to implement something, check how KDE Plasma and other
vendors do it, in `.vendor/**` first. If the relevant upstream is not there
(e.g. a kwallet question and you need to confirm a fix against its source),
clone that repository into `.vendor/` (shallow: `git clone --depth 1`). Cloning
repositories useful to completing this project is allowed. `.vendor/**` stays
git-ignored, read-only reference, never a dependency or runtime input. Record
the precedent (path + choice) in the commit message or docs.

Commit and push every verified slice automatically; do not leave work uncommitted.
