#!/usr/bin/env bash
# Smallest sufficient cargo check for the crates touched by the current change.
#
# Changed crate = crate whose directory holds a changed file. Its reverse
# dependencies (workspace crates that depend on it, transitively) are checked too,
# because a signature change there breaks them. Root Cargo.toml, Cargo.lock,
# .cargo/** or toolchain changes fall back to the full workspace check.
#
# Usage: scripts/dev-check.sh [BASE_REF]   (default BASE_REF=origin/main)
# Env:   CARGO_BUILD_JOBS (default 2), DEV_CHECK_TESTS=1 also runs `cargo test` for those crates.
set -Eeuo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd -P)"
cd "$ROOT"
BASE="${1:-origin/main}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

changed="$( { git diff --name-only "$BASE"...HEAD 2>/dev/null || true; git diff --name-only HEAD; git ls-files --others --exclude-standard; } | sort -u)"
if [ -z "$changed" ]; then
  echo "dev-check: no changed files vs $BASE; nothing to check"
  exit 0
fi

plan="$(printf '%s\n' "$changed" | python3 -I -c '
import json, subprocess, sys
changed = [l.strip() for l in sys.stdin if l.strip()]
meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"]))
ws = {p["name"]: p for p in meta["packages"]}
# Reverse-dependency map over workspace packages (normal, build and dev edges).
rdeps = {n: set() for n in ws}
for n, p in ws.items():
    for d in p["dependencies"]:
        if d["name"] in rdeps:
            rdeps[d["name"]].add(n)
dirs = {n: p["manifest_path"].rsplit("/", 1)[0] for n, p in ws.items()}
root_sensitive = any(f in ("Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml") or f.startswith(".cargo/") for f in changed)
if root_sensitive:
    print("ALL")
    sys.exit(0)
direct = set()
for f in changed:
    for n, d in dirs.items():
        rel = d.split("/crates/", 1)[-1] if "/crates/" in d else None
        if rel and f.startswith("crates/" + rel + "/"):
            direct.add(n)
if not direct:
    print("NONE")
    sys.exit(0)
closure = set(direct)
todo = list(direct)
while todo:
    c = todo.pop()
    for r in rdeps.get(c, ()):
        if r not in closure:
            closure.add(r)
            todo.append(r)
print(" ".join(sorted(closure)))
')"

case "$plan" in
  ALL)
    echo "dev-check: root build config changed; running full workspace check"
    cargo check --workspace --all-targets
    ;;
  NONE)
    echo "dev-check: changed files are outside crates/ (no crate check needed)"
    ;;
  *)
    echo "dev-check: crates = $plan"
    args=()
    for c in $plan; do args+=(-p "$c"); done
    cargo check "${args[@]}" --all-targets
    if [ "${DEV_CHECK_TESTS:-0}" = "1" ]; then
      cargo test "${args[@]}"
    fi
    ;;
esac

python3 -I tools/architecture_guard.py >/dev/null
echo "OK dev-check (architecture guard passed)"
