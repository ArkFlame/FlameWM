# FlameWM Standing Goal Loop

Status: standing operating procedure for autonomous work on `main`.
Scope: this file governs how work is planned, delegated, gated and landed.
It does not change product architecture; `docs/ARCHITECTURE.md` and
`docs/ARCHITECTURE_EXCEPTIONS.toml` still govern product structure.

## 1. Roles

| Role | Who | May do | May not do |
| --- | --- | --- | --- |
| ORCHESTRATOR | Primary agent | Scope briefs, launch workers, verify results, reconcile backlog, land commits | Feature work, refactors, multi-file edits |
| CODER | `general` worker, `Role: CODER` | One independent deliverable in a private worktree | Launch other agents, edit `docs/BACKLOG.md`, touch the main checkout |

- The orchestrator edits files only for one-line fixes: one line, one value,
  one rename, one config tweak.
- Anything spanning several files, or any feature or refactor, goes to a CODER.
- No recursive delegation. A CODER never launches another agent.

## 2. Parallelism

- Run exactly **4** `general` CODER workers in parallel at any time.
- Relaunch a new worker the moment one ends. Do not batch waves; keep four slots full
  while work remains in the queue.
- Workers run in the foreground unless the task explicitly asks for background.
- Serialize overlapping work:
  - two deliverables that touch the same file run one after the other;
  - shared build commands (`scripts/gate`, `scripts/lint`, `cargo` on the shared
    target directory) run one at a time. `scripts/gate` already serializes through
    its cargo lock; do not work around it.

## 3. Brief format

Every CODER brief contains:

1. `Role: CODER`
2. Task, in one or two sentences.
3. Scope: exact files or crates allowed; everything else is off limits.
4. Known evidence: file paths, line references, failing output, and the first
   wrong state observed.
5. Reference precedent: vendor path and the choice taken (see section 6).
6. Acceptance: the gate command and the observable result that proves the task.

The orchestrator reads files only to scope briefs and to verify results. It does
not re-read files a worker has already changed; it verifies through checks.

## 4. Worktrees

- Each worker, and the orchestrator for landings, works in a **private worktree**:

  ```text
  git fetch && git worktree add /tmp/opencode/wt-<slug> origin/main
  ```

- `/tmp/opencode/wt-*` directories from earlier runs are stale; ignore them.
- Never touch the main checkout. It may be dirty with work that is not yours.
- Never run `git stash`, `git reset`, or any command that rewrites another
  worker's state.
- Remove a worktree only after its commit is on `origin/main`.

## 5. Safety limits

- **Never probe host system buses.** Do not run `busctl`, `gdbus`, or any
  read-only probe against the host D-Bus or polkit. A read-only
  `CheckAuthorization` call has previously aborted the host `polkitd`.
- Disk floor: stop and report if the repository filesystem has less than
  **20 GB** free, or `/tmp` has less than **10 GB** free.
- Cargo: `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2` at most.

## 6. Reference-first design

When a choice is unclear, do not ask. Inspect how the reference implementations
solve it, adapt the result to FlameWM architecture, and cite the precedent.

Reference sources (all under `.vendor/`, present in the main checkout):

| Area | Reference paths |
| --- | --- |
| Desktop shell, settings, panels | `.vendor/cosmic-epoch`, `.vendor/LingmoOS-main`, `.vendor/mate-desktop-master` |
| Compositor and Wayland core | `.vendor/smithay-master`, `.vendor/cosmic-epoch` |
| Window manager behaviour | `.vendor/xfwm4-master`, `.vendor/icewm-master` |
| Desktop icons, wallpaper | `.vendor/xfdesktop-master` |
| Standards | freedesktop.org / XDG specs, WAI-ARIA, polkit |
| Other apps | `.vendor/starling-main`, `.vendor/vela-main`, and the others listed in `.vendor/` |

Rules:

- `.vendor/**` is **read-only reference**. Never add it as a dependency, never
  build from it, never copy files verbatim.
- Re-implement the chosen behaviour in FlameWM code.
- Check the licence of the source before borrowing an idea. Record it in the
  commit body. Some vendor trees lack a top-level licence file (for example
  `cosmic-epoch`); treat those as "idea only, no code".
- Cite each precedent in the commit body as `vendor path + choice`, for example:

  ```text
  Precedent: .vendor/xfwm4-master (focus follows click); chose the same rule.
  ```

- Blocked only for a true safety or legal impossibility, and state the reason.

## 7. Evidence rules

- Unit tests prove wiring only. They do not prove runtime behaviour.
- Runtime claims (the app runs, a window appears, input works) need executed
  evidence from a real run, with the command and its exit code.
- Static inspection is not proof.
- Never claim a check that did not run. A check that never ran is not a pass.

## 8. Gate

Every landing passes the scoped gate for each changed crate:

```text
FLAMEWM_TOOLCHAIN_BIN=/home/linsaftw/.rustup/toolchains/1.89.0-x86_64-unknown-linux-gnu/bin \
CARGO_HOME=/tmp/opencode/<home> CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 \
GATE_LOG=/tmp/opencode/<slug>-gate.log scripts/gate <crate>
```

Pass condition: every `EXIT_*` line in the log is `0`, and the log ends with
`GATE_DONE all=0`.

`scripts/gate` runs the architecture guard, `cargo fmt --check`, clippy with
`-D warnings`, and tests for the named crates.

## 9. Landing

Land each slice in real time, one slice at a time:

1. Commit in the private worktree.
2. `git fetch origin`, then rebase onto `origin/main`.
3. If the rebase changed the crate closure of the slice, re-run the gate.
4. Push fast-forward only:

   ```text
   git push origin HEAD:main
   ```

   A non-fast-forward rejection means re-fetch and repeat from step 2. Never force.

Commit message rules:

- Subject: `<type>: <summary>`.
- Body: the precedent citation (section 6) and the gate exit lines.
- Trailer, exactly:

  ```text
  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
  ```

## 10. Backlog

- Workers do not edit `docs/BACKLOG.md`.
- The orchestrator reconciles `docs/BACKLOG.md` after each landing, from the
  worker reports and the commit SHAs.

## 11. Verification of landings

Before reporting a slice as landed:

- `git merge-base --is-ancestor <sha> origin/main` exits `0`.
- `git log -1 --format=%B <sha>` contains the `Co-Authored-By` trailer.
- The gate log contains only `EXIT_*=0` lines and `GATE_DONE all=0`.

Report the SHA and the exit codes.

## 12. Stop conditions

Stop the loop and report when:

- the disk floor in section 5 is reached;
- a true safety or legal impossibility blocks a slice (state the reason);
- the work queue is empty.

Otherwise the loop continues without asking the user questions.
