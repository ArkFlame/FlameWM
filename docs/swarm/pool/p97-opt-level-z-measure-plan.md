# Unit p97-opt-level-z-measure-plan
STATUS: DONE

Uncommitted Cargo.toml changes opt-level "s" -> "z" for resident apps and add a flamewm-terminal profile (from p81). Read the diff (git diff Cargo.toml) and docs/swarm/pool/p81-release-profile-size.md. Write docs/OPT_LEVEL_PLAN.md: per package, expected size delta, expected CPU effect (z is slower than s for hot loops), which packages are hot (render paths stay opt-level 3), and an exact measurement recipe using scripts/measure-panic-profile size mode and scripts/bench-baseline. Recommend keep/revert per package. No Cargo.toml edits.

## Result

Wrote `docs/OPT_LEVEL_PLAN.md`: per-package table (hot vs UI-only), expected effects marked unmeasured, a keep/revert rule, and a 6-step recipe using `scripts/measure-panic-profile size|rss` and `scripts/bench-baseline`. Recommendation: keep `z` on the eight UI-only bins pending measurement; keep `s` on wm, desktop, shell, lock, apps, terminal. No Cargo.toml edits, no git, no builds, no measurements run. The git diff was not read (no-git rule); base values come from p81.
