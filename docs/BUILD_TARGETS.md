# Build Target Dirs Per Worktree

## Evidence (2026-10-31)

- Login shell exports `CARGO_TARGET_DIR=$HOME/.cache/cargo-target` (`~/.zshrc:77`). That env var overrides every `.cargo/config.toml` `[build] target-dir`, so all worktrees share one target.
- Cargo hashes path packages relative to the workspace root. Identical relative layouts therefore produce identical metadata hashes. Same-named artifacts such as `libflamewm_render_core-<hash>.rmeta` are shared across worktrees.
- Dep-info (`.d`) files list workspace-relative `crates/flamewm-render-core/src/*.rs` paths, so they do not identify the worktree.
- Two artifacts with the same hash were found in both `~/.cache/cargo-target/debug/deps` and `flamewm-targets/rel/debug/deps` (e.g. `774aa9ea854877ef`, `c12cdb6f5194cc2c`, `d297613af94bfbb7`, `f5c5267465e66c6c`, `a149d08b3188faa1`).
- Result: a build in one worktree can read another worktree's `rmeta`. That produced `E0004 CursorKind::Busy not covered` in `flamewm-render-x11`.

## Rule

1. Every worktree uses its own target dir. Never share `CARGO_TARGET_DIR` between worktrees.
2. Target dir for worktree `<name>`: `/run/media/linsaftw/SDD/flamewm-targets/wt/<name>` (SDD disk, not `/`, which is near full).
3. Set it per command, not in shell startup:
   `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/flamewm-targets/wt/<name> CARGO_BUILD_JOBS=2 flock /run/media/linsaftw/SDD/flamewm-targets/.lock cargo check -p flamewm-render-x11`
4. Do not add `CARGO_TARGET_DIR` to `~/.zshrc` globally. Keep the global export out of login shells.
5. If a build reports an impossible `E0004`/`E0432` on a symbol that the worktree's source lacks, stop. Check `cargo metadata` and the target dir before editing source.
6. Shared-target cleanup (only with the lock held): delete `flamewm_render_core-*` and `flamewm_render_x11-*` under the affected dir and rebuild. Do not delete other workers' target dirs.

## Verify

```
cd <worktree> && cargo metadata --format-version 1 --no-deps | jq -r '.target_directory'
```
The output must be the worktree's own `wt/<name>` dir.
