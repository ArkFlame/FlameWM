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

# The probe is itself a pending start job, so is-system-running can never leave "starting"
# while it runs. Settled = no pending jobs besides this unit and its parent targets, and no
# failed units. Bounded at 90s; reports "timeout" if other jobs are still pending.
settle=timeout
for _ in $(seq 1 90); do
  pending=$(systemctl list-jobs --no-legend 2>/dev/null | awk '{print $2}' \
    | grep -vxE 'flame-probe.service|multi-user.target|graphical.target' || true)
  if [[ -z $pending ]]; then
    if [[ -n $(systemctl --failed --no-legend 2>/dev/null) ]]; then settle=degraded; else settle=running; fi
    break
  fi
  sleep 1
done

emit FLAME_PROBE_BEGIN
emit "lightdm_active=$active"
emit "boot_seconds=$(cut -d' ' -f1 /proc/uptime)"
emit "systemd_state=$settle"
emit "systemd_analyze=$(systemd-analyze 2>/dev/null | tr '\n' ' ')"
emit "systemd_jobs=$(systemctl list-jobs --no-legend 2>/dev/null | awk '{printf "%s:%s;", $2, $3}')"
emit "systemd_failed=$(systemctl --failed --no-legend 2>/dev/null | awk '{printf "%s;", $2}')"
emit "systemd_blame=$(systemd-analyze blame 2>/dev/null | head -n 10 | awk '{printf "%s %s;", $1, $2}')"
emit "systemd_critical=$(systemd-analyze critical-chain 2>/dev/null | tr -s ' \n' ' ' | cut -c1-800)"
# power-profiles-daemon: exercise the D-Bus activation path FlameWM uses, with a bounded wait,
# then record why it did or did not start (unit state, journal tail, status).
emit "ppd_enabled=$(systemctl is-enabled power-profiles-daemon.service 2>&1 | tr '\n' ' ')"
ppd_out=$(timeout 10 busctl --system --timeout=3 get-property org.freedesktop.UPower.PowerProfiles /net/hadess/PowerProfiles net.hadess.PowerProfiles ActiveProfile 2>&1); ppd_rc=$?
emit "ppd_activate_rc=$ppd_rc"
emit "ppd_activate_out=$(printf '%s' "$ppd_out" | tr '\n' ' ' | cut -c1-200)"
emit "ppd_state=$(systemctl is-active power-profiles-daemon.service 2>&1 | tr '\n' ' ')"
emit "ppd_jobs_after=$(systemctl list-jobs --after --no-legend 2>/dev/null | tr -s ' \n' ' ' | cut -c1-600)"
emit "ppd_deps=$(systemctl show power-profiles-daemon.service -p Requires -p Wants -p BindsTo -p After -p Before -p Conflicts -p JobTimeoutUSec 2>/dev/null | tr '\n' ' ' | cut -c1-500)"
emit "ppd_verify=$(systemd-analyze verify /usr/lib/systemd/system/power-profiles-daemon.service 2>&1 | tr '\n' ' ' | cut -c1-300)"
emit "ppd_journal=$(journalctl -b -u power-profiles-daemon.service --no-pager -o cat 2>/dev/null | tail -n 12 | tr '\n' '|' | cut -c1-900)"
emit "ppd_status=$(systemctl status power-profiles-daemon.service --no-pager -l 2>/dev/null | tr '\n' '|' | cut -c1-700)"
emit "udisks2_rss_mb=$(ps -C udisksd -o rss= 2>/dev/null | awk '{s+=$1} END{printf "%d", s/1024}')"
emit "bluetoothd_rss_mb=$(ps -C bluetoothd -o rss= 2>/dev/null | awk '{s+=$1} END{printf "%d", s/1024}')"
emit "ppd_rss_mb=$(ps -eo rss,args --no-headers 2>/dev/null | awk '/power-profiles-daemon/ && !/awk/{s+=$1} END{printf "%d", s/1024}')"
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
