#!/bin/bash
# flame-probe.sh - FlameDE TEST-ONLY guest probe. Installed into test ISOs only, never release.
# Waits up to 300s for lightdm, then prints key=value metrics between FLAME_PROBE_BEGIN/END
# on ttyS0 (serial console). Consumed by distro/tests/boot-smoke.sh.
set -u

emit() { printf '%s\n' "$*" >/dev/ttyS0 2>/dev/null || printf '%s\n' "$*"; }

active=0
deadline=$((SECONDS + 300))
while (( SECONDS < deadline )); do
  if systemctl is-active --quiet lightdm; then active=1; break; fi
  sleep 2
done
if (( active )); then emit FLAME_LIGHTDM_ACTIVE; fi

emit FLAME_PROBE_BEGIN
emit "lightdm_active=$active"
emit "boot_seconds=$(cut -d' ' -f1 /proc/uptime)"
emit "systemd_state=$(systemctl is-system-running 2>/dev/null)"
emit "systemd_analyze=$(systemd-analyze 2>/dev/null | tr '\n' ' ')"
read -r mem_total mem_used mem_avail < <(free -m | awk '/^Mem:/{print $2, $3, $7}')
emit "mem_total_mb=$mem_total"
emit "mem_used_mb=$mem_used"
emit "mem_available_mb=$mem_avail"
emit "services_running=$(systemctl list-units --type=service --state=running --no-legend --plain | wc -l)"
emit "top_rss=$(ps -eo comm,rss --sort=-rss --no-headers | head -n 10 |
  awk '{gsub(/,/, "", $1); printf "%s:%d,", $1, $2 / 1024}' | sed 's/,$//')"
emit FLAME_PROBE_END
