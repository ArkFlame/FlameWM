#!/bin/bash
# flame-probe.sh - FlameDE TEST-ONLY guest probe. Installed into test ISOs only, never release.
# Waits up to 300s for lightdm, then waits (bounded) for systemd to settle so the state is real,
# then prints key=value metrics between FLAME_PROBE_BEGIN/END on ttyS0 (serial console).
# Consumed by distro/tests/boot-smoke.sh. Extra diagnostics (blame, critical-chain, full RSS list)
# are emitted as single lines; boot-smoke.sh copies them into distro/out/boot-serial.log.
# Unit TimeoutStartSec=400 in flame-probe.service: 300s lightdm wait + 90s settle stays below it.
set -u

emit() { printf '%s\n' "$*" >/dev/ttyS0 2>/dev/null || printf '%s\n' "$*"; }

active=0
deadline=$((SECONDS + 300))
while (( SECONDS < deadline )); do
  if systemctl is-active --quiet lightdm; then active=1; break; fi
  sleep 2
done
if (( active )); then emit FLAME_LIGHTDM_ACTIVE; fi

# is-system-running --wait blocks until the state leaves starting/initializing; bounded by timeout.
settle=$(timeout 90 systemctl is-system-running --wait 2>/dev/null || true)
[[ -n $settle ]] || settle=timeout

emit FLAME_PROBE_BEGIN
emit "lightdm_active=$active"
emit "boot_seconds=$(cut -d' ' -f1 /proc/uptime)"
emit "systemd_state=$settle"
emit "systemd_analyze=$(systemd-analyze 2>/dev/null | tr '\n' ' ')"
emit "systemd_jobs=$(systemctl list-jobs --no-legend 2>/dev/null | awk '{printf "%s:%s;", $2, $3}')"
emit "systemd_failed=$(systemctl --failed --no-legend 2>/dev/null | awk '{printf "%s;", $2}')"
emit "systemd_blame=$(systemd-analyze blame 2>/dev/null | head -n 10 | awk '{printf "%s %s;", $1, $2}')"
emit "systemd_critical=$(systemd-analyze critical-chain 2>/dev/null | tr -s ' \n' ' ' | cut -c1-800)"
read -r mem_total mem_used mem_avail < <(free -m | awk '/^Mem:/{print $2, $3, $7}')
emit "mem_total_mb=$mem_total"
emit "mem_used_mb=$mem_used"
emit "mem_available_mb=$mem_avail"
emit "services_running=$(systemctl list-units --type=service --state=running --no-legend --plain | wc -l)"
emit "rss_total_mb=$(ps -eo rss --no-headers | awk '{s+=$1} END{printf "%d", s/1024}')"
emit "top_rss=$(ps -eo comm,rss --sort=-rss --no-headers | head -n 10 |
  awk '{gsub(/,/, "", $1); printf "%s:%d,", $1, $2 / 1024}' | sed 's/,$//')"
emit "rss_all=$(ps -eo comm,rss --sort=-rss --no-headers | head -n 40 |
  awk '{gsub(/,/, "", $1); printf "%s:%d,", $1, $2 / 1024}' | sed 's/,$//')"
emit FLAME_PROBE_END
