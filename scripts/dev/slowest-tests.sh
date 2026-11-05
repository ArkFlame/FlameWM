#!/usr/bin/env bash
# Print the slowest tests of one crate, from the libtest JSON report.
# Usage: scripts/dev/slowest-tests.sh <crate> [<count>]   (default count 15)
# Builds the crate's test binaries (dev profile, shared target dir), runs each one, and ranks tests
# by libtest exec_time. The report flag is nightly-only, so RUSTC_BOOTSTRAP=1 is set for the test
# binary only. Failing tests do not stop the ranking; their stderr stays visible.
set -euo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
. "$ROOT/scripts/dev/env.sh"
[ $# -ge 1 ] || { echo "usage: $0 <crate> [count]" >&2; exit 2; }
crate="$1"
count="${2:-15}"
cd "$ROOT"
report="$(mktemp)"
"${FLAME_NICE[@]}" cargo test -p "$crate" --no-run --message-format=json 2>/dev/null \
  | python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if msg.get("reason") == "compiler-artifact" and msg.get("profile", {}).get("test") and msg.get("executable"):
        print(msg["executable"])
' > "$report.bins"
trap 'rm -f "$report" "$report.bins"' EXIT
[ -s "$report.bins" ] || { echo "no test binaries for $crate" >&2; exit 1; }
while read -r bin; do
  RUSTC_BOOTSTRAP=1 "${FLAME_NICE[@]}" "$bin" -Zunstable-options --report-time --format json \
    --test-threads="$RUST_TEST_THREADS" >> "$report" || true
done < "$report.bins"
python3 -I -c '
import json, sys
rows = []
for line in open(sys.argv[1]):
    if not line.startswith("{"):
        continue
    d = json.loads(line)
    if d.get("type") == "test" and "exec_time" in d:
        rows.append((float(d["exec_time"]), d["name"], d["event"]))
rows.sort(reverse=True)
print(f"{len(rows)} tests, {sum(r[0] for r in rows):.2f}s summed exec_time")
for secs, name, event in rows[: int(sys.argv[2])]:
    print(f"{secs:9.3f}s  {event:6s}  {name}")
' "$report" "$count"
