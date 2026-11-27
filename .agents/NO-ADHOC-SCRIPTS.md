# Blacklisted: python, ctx_execute, temp scripts, sleep polling (user order, permanent)

Read `.agents/skills/no-adhoc-scripts/SKILL.md`. Summary: never run python/python3/ctx_execute or `/tmp/ctx-exec-*/code.py`; they overload the machine. Use rg/fd/sed/jq/Edit and reusable bash or Rust in `./scripts/` (`scripts/perf-loop` for status, wait, sync, check). Violations: stop the process, tell the user.
