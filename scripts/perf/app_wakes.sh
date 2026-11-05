#!/usr/bin/env bash
# app_wakes.sh: per-process idle wake sampler (voluntary + involuntary ctxt switches per second).
# usage: app_wakes.sh <seconds> <pid|comm> [<pid|comm> ...]
#   pid   : sample that pid (all its tasks)
#   comm  : sample every pid whose /proc/<pid>/comm equals the name (exact match)
# Output: one line per matched pid: pid comm voluntary/s involuntary/s total/s
# Reads /proc/<pid>/task/*/status only; never signals anything.
set -u
[ $# -ge 2 ] || { echo "usage: app_wakes.sh <seconds> <pid|comm> [...]" >&2; exit 2; }
SECS=$1; shift

pids_for(){
  local sel=$1
  if [[ $sel =~ ^[0-9]+$ ]]; then
    [ -d "/proc/$sel" ] && echo "$sel"
  else
    for d in /proc/[0-9]*; do
      [ "$(cat "$d/comm" 2>/dev/null)" = "$sel" ] && echo "${d#/proc/}"
    done
  fi
}

snap(){
  # prints: pid comm vol invol
  local pid=$1 comm vol=0 invol=0 line
  comm=$(cat "/proc/$pid/comm" 2>/dev/null) || return 1
  for t in /proc/"$pid"/task/*/status; do
    while IFS= read -r line; do
      case $line in
        voluntary_ctxt_switches:*) vol=$((vol + ${line##*:})) ;;
        nonvoluntary_ctxt_switches:*) invol=$((invol + ${line##*:})) ;;
      esac
    done < "$t" 2>/dev/null
  done
  echo "$pid $comm $vol $invol"
}

declare -A seen
targets=()
for sel in "$@"; do
  for p in $(pids_for "$sel"); do
    [ -n "${seen[$p]:-}" ] && continue
    seen[$p]=1; targets+=("$p")
  done
done
[ ${#targets[@]} -gt 0 ] || { echo "no matching processes" >&2; exit 1; }

declare -A v0 i0 c0
for p in "${targets[@]}"; do
  read -r _ c v i < <(snap "$p") || continue
  v0[$p]=$v; i0[$p]=$i; c0[$p]=$c
done
t0=$(date +%s.%N)
sleep "$SECS"
t1=$(date +%s.%N)
el=$(python3 -I -c 'import sys; print(float(sys.argv[1])-float(sys.argv[2]))' "$t1" "$t0")

printf '%-8s %-22s %12s %12s %12s\n' pid comm vol/s invol/s total/s
for p in "${targets[@]}"; do
  [ -n "${v0[$p]:-}" ] || continue
  read -r _ c v i < <(snap "$p") || continue
  python3 -I -c 'import sys; p,c,e,v0,v1,i0,i1=sys.argv[1:]; e=float(e); dv=int(v1)-int(v0); di=int(i1)-int(i0); print(f"{p:<8} {c:<22} {dv/e:>12.2f} {di/e:>12.2f} {(dv+di)/e:>12.2f}")' \
    "$p" "$c" "$el" "${v0[$p]}" "$v" "${i0[$p]}" "$i"
done
