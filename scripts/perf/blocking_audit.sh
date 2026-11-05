#!/usr/bin/env bash
# Ranked audit of blocking calls in product source (UI / compositor / event-loop owners).
# Heuristic grep: each hit gets a kind weight, a thread hint and a frequency/stall estimate.
# Output: markdown table on stdout. Usage: scripts/perf/blocking_audit.sh [TOP_N]
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
TOP_N="${1:-40}"
SCOPE=(
  crates/flamewm-platform/src
  crates/flamewm-shell/src
  crates/flamewm-session-core/src
  crates/flamewm-integrations-linux/src
  crates/flamewm-lock/src
  crates/flamewm-control-dbus/src
)
# Excluded by the owner rules: settings services and icon theme, startup/colour code, other families.
EXCLUDE_RE='services/settings|host\.rs$|icon_theme|icon_loader|icon_service|icons/|startup|colou?r|/tests?\.rs:|/tests/|_tests?\.rs:|template_tests|/build\.rs:'

PATTERNS='sync_all|sync_data|\.output\(\)|\.status\(\)|\.wait\(\)|wait_with_output|thread::sleep|fs::(read|read_to_string|write|copy|rename|create_dir_all|remove_file)\(|File::(open|create)\(|recv\(\)|recv_timeout\('

cd "$ROOT"
hits="$(grep -rnE "$PATTERNS" "${SCOPE[@]}" --include='*.rs' | grep -vE "$EXCLUDE_RE" || true)"

# kind|weight|stall
classify() {
  case "$1" in
    *sync_all*|*sync_data*)            echo "fsync|5|fsync: 1-50 ms, unbounded on loaded disk" ;;
    *output\(\)*|*status\(\)*|*wait\(\)*|*wait_with_output*) echo "child-wait|5|process spawn+wait: 5-500 ms" ;;
    *recv\(\)*|*recv_timeout*)         echo "blocking-recv|4|until sender fires (unbounded or timeout)" ;;
    *thread::sleep*)                   echo "sleep|2|fixed delay per call (no wake)" ;;
    *fs::*|*File::*)                   echo "fs-io|3|syscalls: <1 ms small files, grows with size" ;;
    *)                                 echo "other|1|unknown" ;;
  esac
}

rows="$(while IFS= read -r line; do
  [ -z "$line" ] && continue
  file="${line%%:*}"; rest="${line#*:}"; lineno="${rest%%:*}"; text="${rest#*:}"
  testline="$(grep -m1 -n "^#\[cfg(test)\]" "$file" | cut -d: -f1 || true)"
  if [ -n "$testline" ] && [ "$lineno" -ge "$testline" ]; then continue; fi
  info="$(classify "$text")"
  kind="${info%%|*}"; rest2="${info#*|}"; weight="${rest2%%|*}"; stall="${rest2#*|}"
  fn="$(awk -v n="$lineno" 'NR<=n && /^[[:space:]]*(pub(\([a-z ]+\))? )?(async )?fn /{f=$0} NR==n{print f; exit}' "$file" \
        | sed -E 's/^[[:space:]]*(pub(\([a-z ]+\))? )?(async )?fn ([A-Za-z0-9_]+).*/\4/')"
  if grep -qE 'thread::(Builder|spawn)' "$file"; then thread="worker-file (verify)"; else thread="caller thread (verify)"; fi
  if [[ "$fn" =~ ^(handle|on|dispatch|apply|tick|update|poll|render|paint|process) ]]; then freq="per event/frame"; else freq="once or on demand (verify)"; fi
  mult=2; [[ "$thread" == worker-file* ]] && mult=1
  score=$(( weight * mult ))
  printf '%d|%s:%s|%s|%s|%s|%s|%s\n' "$score" "$file" "$lineno" "$kind" "${fn:-?}" "$thread" "$freq" "$stall"
done <<< "$hits" | sort -t'|' -k1,1nr | head -n "$TOP_N")"

echo "| Rank | file:line | kind | fn | thread | frequency | expected stall | score |"
echo "| ---: | --- | --- | --- | --- | --- | --- | ---: |"
i=0
while IFS='|' read -r score loc kind fn thread freq stall; do
  [ -z "$loc" ] && continue
  i=$((i + 1))
  echo "| $i | \`$loc\` | $kind | \`$fn\` | $thread | $freq | $stall | $score |"
done <<< "$rows"
