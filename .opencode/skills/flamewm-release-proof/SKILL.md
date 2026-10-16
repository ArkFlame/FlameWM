---
name: flamewm-release-proof
description: Prove FlameWM release readiness through fresh build, artifact, and runtime-identity evidence.
---

## Release gate

Release readiness requires fresh executed evidence for applicable build, package/stage, artifact, identity, resource, license, and runtime checks. Source inspection, prior CI, or documentation cannot substitute.

## Procedure

1. Read current version source, release configuration, packaging scripts, and repository-native commands.
2. Build from current worktree with isolated, recorded output paths. Preserve existing artifacts unless command explicitly owns them.
3. Inspect produced artifact names, contents, executable/library linkage as applicable, desktop/resources, licenses/notices, and install layout.
4. Verify shipped version and runtime identity match `scripts/version` (the `Cargo.toml` workspace version) and release metadata.
5. Run required bounded runtime or staged-install proof in suitable isolation; use `flamewm-runtime-evidence` for native behavior.

## Reporting

Report revision/worktree state, commands, exit results, artifact paths and hashes when required, observed version identity, environment, unrun gates, and blocker. A failed or skipped gate is not release-ready.
