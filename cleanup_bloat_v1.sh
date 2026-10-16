#!/usr/bin/env bash
# cleanup_bloat_v1.sh - curated FlameWM repository cleanup.
#
# Modes (default is a dry run; nothing is changed without --apply or --history):
#   (no args)         Step A dry run. Prints the verdict per curated path and
#                     whether --apply would abort. Read-only.
#   --apply           Step A. Stages `git rm` of the curated paths, runs the
#                     architecture guard, promotion check and unittest, then
#                     commits ONLY those paths on main. Any safety failure
#                     aborts before anything is deleted. If a post-delete check
#                     fails, the deletions stay STAGED and nothing is committed.
#                     Recovery is via git history; this script never runs rm.
#   --history-report  Read-only. Lists the 40 largest blobs in all history and
#                     marks each AT-HEAD or HISTORY-ONLY.
#   --history DIR     Step B. Needs `git filter-repo`. Makes a `git clone --mirror`
#                     in DIR (must not exist), writes the history-only blobs
#                     larger than 1 MiB to DIR.ids, then runs
#                     `git filter-repo --strip-blobs-with-ids DIR.ids` in the
#                     mirror only after you type REWRITE on /dev/tty. Blobs
#                     present at HEAD are never listed, so they are never stripped.
#                     This script NEVER pushes. Every commit SHA changes. You must
#                     inspect the mirror and force-push yourself; every clone
#                     must then be re-cloned.
#   --ignored-report  Prints du -sh for ignored dirs (.target .agents/runtime
#                     assets/theme-sources .debug .performance). Deletes nothing.
#   -h, --help        Prints usage.
#
# Step A safety per path: skipped if untracked; aborts the whole apply if
# `git status --porcelain -- <path>` is non-empty, if `git grep -F` finds the
# basename in any other tracked file, or if the path or basename appears in
# packaging/source-manifest.txt or MANIFEST.sha256. Step A refuses a branch
# other than main unless FORCE=1, and refuses staged work. The commit uses
# `git commit -- <paths>` so nothing else is included. Paths are explicit;
# no globs and no `git add -A`.
#
# Known gap: the guard may fail only because crates/flamewm-session-core/src/
# allocator.rs is untracked. The script prints a note but does not delete it.
set -euo pipefail

readonly COMMIT_MSG='chore: remove unreferenced probes and stray trace CSVs'
readonly COAUTHOR='Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>'
readonly BIG_BYTES=1048576
readonly CURATED=(
  move-trace.csv
  t06-move-trace.csv
  t06-move-trace-3.csv
  tools/flamewm_g8_mute_osd_probe.py
  tools/flamewm_gtk3_e2e_probe.py
  tools/flamewm_outside_click_probe.py
  tools/flamewm_battery_probe.py
  tools/flamewm_audio_popup_probe.py
  tools/flamewm_appearance_toolkit_probe.py
)
readonly IGNORED_DIRS=(.target .agents/runtime assets/theme-sources .debug .performance)

die() { echo "ABORT: $*" >&2; exit 1; }
note() { echo "NOTICE: $*"; }

usage() {
  cat >&2 <<'USAGE'
usage: cleanup_bloat_v1.sh [--apply | --history-report | --history DIR | --ignored-report | -h]
  no args           dry run of step A (read-only, default)
  --apply           perform step A and commit on main
  --history-report  read-only report of the 40 largest blobs in history
  --history DIR     step B: rewrite a mirror clone in DIR (never pushes)
  --ignored-report  du -sh of ignored dirs (deletes nothing)
USAGE
  exit "${1:-2}"
}

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "not inside a git repository"
cd "$ROOT"

# Prints a verdict for one curated path. Returns 0 = delete, 2 = skip, 1 = abort.
vet_path() {
  local p=$1 base refs m
  base=$(basename -- "$p")
  if ! git ls-files --error-unmatch -- "$p" >/dev/null 2>&1; then
    note "$p is not tracked; skipping"
    return 2
  fi
  if [[ -n $(git status --porcelain -- "$p") ]]; then
    echo "ABORT: $p has uncommitted changes" >&2
    return 1
  fi
  refs=$(git grep -lF -e "$base" -- . ":(exclude,literal)$p" | tr '\n' ' ' || true)
  if [[ -n $refs ]]; then
    echo "ABORT: '$base' is still referenced by tracked files: $refs" >&2
    return 1
  fi
  for m in packaging/source-manifest.txt MANIFEST.sha256; do
    if [[ -f $m ]] && grep -qF -e "$p" -e "$base" -- "$m"; then
      echo "ABORT: $p (or '$base') appears in $m" >&2
      return 1
    fi
  done
  echo "ok: $p would be removed"
  return 0
}

# Fills TO_DELETE; returns 1 if any path aborts.
vet_all() {
  local p rc failed=0
  TO_DELETE=()
  for p in "${CURATED[@]}"; do
    rc=0
    vet_path "$p" || rc=$?
    case $rc in
      0) TO_DELETE+=("$p") ;;
      2) ;;
      *) failed=1 ;;
    esac
  done
  return $failed
}

run_check() {
  local name=$1 rc=0
  shift
  echo "== $name: $*"
  "$@" || rc=$?
  if (( rc == 0 )); then
    echo "== $name: PASS (exit 0)"
  else
    echo "== $name: FAIL (exit $rc)"
  fi
  return "$rc"
}

cmd_dry_run() {
  local rc=0
  echo "DRY RUN (step A). Nothing is changed. Branch: $(git rev-parse --abbrev-ref HEAD)"
  if git diff --cached --quiet; then
    echo "preflight: no staged changes"
  else
    echo "preflight: STAGED changes present (--apply would refuse)"
  fi
  vet_all || rc=1
  if (( ${#TO_DELETE[@]} )); then
    echo "Would git rm (${#TO_DELETE[@]}):"
    printf '  %s\n' "${TO_DELETE[@]}"
  fi
  if (( rc )); then
    echo "--apply would ABORT (see above). Fix before running it."
    exit 1
  fi
  echo "To perform step A, run: ./cleanup_bloat_v1.sh --apply"
}

cmd_apply() {
  local rc=0 br allocator=crates/flamewm-session-core/src/allocator.rs
  br=$(git rev-parse --abbrev-ref HEAD)
  if [[ $br != main && ${FORCE:-0} != 1 ]]; then
    die "branch is '$br', not main (set FORCE=1 to override)"
  fi
  git diff --cached --quiet || die "staged changes present; unstage them first"
  vet_all || die "safety check failed; nothing was deleted"
  if (( ${#TO_DELETE[@]} == 0 )); then
    note "nothing to remove"
    exit 0
  fi
  git rm -q -- "${TO_DELETE[@]}"
  echo "Staged removal of ${#TO_DELETE[@]} path(s)."

  run_check architecture_guard python3 -I tools/architecture_guard.py || rc=1
  run_check verify_rwr_promotion python3 -I tools/verify_rwr_promotion.py || rc=1
  run_check unittest python3 -I -m unittest discover -s tools -p 'test_*.py' || rc=1
  if (( rc )); then
    if [[ -e $allocator ]] && ! git ls-files --error-unmatch -- "$allocator" >/dev/null 2>&1; then
      note "$allocator is untracked and may be why the guard fails. It was NOT deleted."
    fi
    die "post-delete checks failed. Deletions are STAGED but NOT committed. Unstage with: git restore --staged -- ${TO_DELETE[*]}"
  fi

  git commit -q -m "$COMMIT_MSG" -m "$COAUTHOR" -- "${TO_DELETE[@]}"
  git log -1 --stat --format='%h %s'
}

# Prints "SHA SIZE_BYTES PATH" for every blob in repo $1, largest first.
blob_inventory() {
  git -C "$1" rev-list --objects --all \
    | git -C "$1" cat-file --batch-check='%(objecttype) %(objectname) %(objectsize) %(rest)' \
    | awk '$1 == "blob" { p = $4; for (i = 5; i <= NF; i++) p = p " " $i; print $2, $3, p }' \
    | sort -k2,2nr
}

# Prints "STATUS SIZE_BYTES SHA PATH" for blobs of repo $1; AT-HEAD means present in the working repo's HEAD.
annotate() {
  awk 'FNR == NR { h[$3] = 1; next }
       { p = $3; for (i = 4; i <= NF; i++) p = p " " $i
         print ($1 in h ? "AT-HEAD" : "HISTORY-ONLY"), $2, $1, p }' \
    <(git -C "$ROOT" ls-tree -r HEAD) <(blob_inventory "$1")
}

cmd_history_report() {
  echo "Top 40 blobs in all history (STATUS SIZE_BYTES SHA PATH):"
  annotate "$ROOT" | sed -n '1,40p'
  annotate "$ROOT" | awk -v big="$BIG_BYTES" '
    $1 == "HISTORY-ONLY" && $2 > big { n++; b += $2 }
    END { printf "History-only blobs > %d bytes: %d (%.1f MiB total)\n", big, n + 0, b / 1048576 }'
  echo "Read-only. Nothing was changed."
}

cmd_history() {
  local dir=$1 ids
  if ! git filter-repo --version >/dev/null 2>&1; then
    cat >&2 <<'INSTALL'
git filter-repo is not installed. Install it, for example:
  pip install git-filter-repo        (or your distro package "git-filter-repo")
then rerun: ./cleanup_bloat_v1.sh --history DIR
INSTALL
    exit 1
  fi
  [[ ! -e $dir && ! -e ${dir}.ids ]] || die "$dir or ${dir}.ids already exists"
  git clone -q --mirror "$ROOT" "$dir"
  dir=$(cd "$dir" && pwd)
  ids="${dir}.ids"

  echo "Mirror created at: $dir (working repo is untouched)"
  echo "Size before: $(du -sh -- "$dir" | cut -f1)"
  echo "Top 40 blobs in all history (STATUS SIZE_BYTES SHA PATH):"
  annotate "$dir" | sed -n '1,40p'

  annotate "$dir" | awk -v big="$BIG_BYTES" '$1 == "HISTORY-ONLY" && $2 > big { print $3 }' > "$ids"
  if [[ ! -s $ids ]]; then
    note "no history-only blobs above $BIG_BYTES bytes; nothing to strip"
    exit 0
  fi
  # Defensive: no id to strip may be a blob at HEAD.
  if git ls-tree -r HEAD | awk 'FNR == NR { s[$1] = 1; next } ($3 in s) { print }' "$ids" - | grep -q .; then
    die "an id to strip is present at HEAD; refusing"
  fi
  echo "Blob ids to strip ($(wc -l < "$ids")) written to: $ids"

  cat <<WARN

WARNING: this REWRITES every commit in the MIRROR at:
  $dir
- Strips only the history-only blobs above $BIG_BYTES bytes listed in $ids.
- Blobs present at HEAD are not touched.
- Every commit SHA changes. This script never pushes.
WARN
  if ! read -r -p 'Type REWRITE to continue: ' answer </dev/tty; then
    die "no terminal to confirm on (/dev/tty)"
  fi
  [[ $answer == REWRITE ]] || die "not confirmed; mirror left unchanged"

  git -C "$dir" filter-repo --strip-blobs-with-ids "$ids"
  echo "Size after: $(du -sh -- "$dir" | cut -f1)"
  cat <<STEPS

MANUAL STEPS (you run these; this script did NOT push):
  1. Inspect the mirror: git -C "$dir" log --oneline --all | head
  2. Verify the rewritten history and that the HEAD tree is unchanged.
  3. Force-push yourself: git -C "$dir" push --mirror <remote-url>
     (this overwrites remote history; coordinate first).
  4. Every commit SHA has changed. Every existing clone must be
     re-cloned; old clones keep the old history.
STEPS
}

cmd_ignored_report() {
  local d
  for d in "${IGNORED_DIRS[@]}"; do
    if [[ -e $d ]]; then
      du -sh -- "$d" 2>/dev/null || true
    else
      echo "(absent) $d"
    fi
  done
  echo "These paths are gitignored. This script does NOT delete them."
}

main() {
  case ${1:-} in
    "") (( $# <= 1 )) || usage; cmd_dry_run ;;
    --apply) (( $# == 1 )) || usage; cmd_apply ;;
    --history-report) (( $# == 1 )) || usage; cmd_history_report ;;
    --history) (( $# == 2 )) || usage; cmd_history "$2" ;;
    --ignored-report) (( $# == 1 )) || usage; cmd_ignored_report ;;
    -h | --help) usage 0 ;;
    *) usage ;;
  esac
}

main "$@"
