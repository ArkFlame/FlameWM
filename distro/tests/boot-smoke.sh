#!/usr/bin/env bash
# boot-smoke.sh [ISO] - boot a FlameDE ISO headless in QEMU and write distro/out/metrics.json.
# Guest contract: the test-only probe in distro/tests/guest/ must be installed in the ISO
# (flame-probe.service -> flame-probe.sh). It prints FLAME_LIGHTDM_ACTIVE once lightdm is active,
# then a key=value block between FLAME_PROBE_BEGIN and FLAME_PROBE_END on ttyS0.
# Env: FW=auto|uefi|bios  BOOT_TIMEOUT=420  MEM=1024  SMP=2  OUT_DIR=<repo>/distro/out
# Exit: 0 PASS, 1 FAILED (boot or probe), 2 precondition missing. metrics.json is always written.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT_DIR=${OUT_DIR:-$REPO/distro/out}
FW=${FW:-auto}
BOOT_TIMEOUT=${BOOT_TIMEOUT:-420}
MEM=${MEM:-1024}
SMP=${SMP:-2}
OUT=$OUT_DIR/metrics.json
SHOT=$OUT_DIR/boot-screen.ppm
mkdir -p "$OUT_DIR"

WORK="" QPID="" SERIAL="" WALL="" MODE="" KVM=0 LOGIN_SEEN=0
# shellcheck disable=SC2329  # invoked via trap
cleanup() {
  if [[ -n $QPID ]]; then kill "$QPID" 2>/dev/null || true; wait "$QPID" 2>/dev/null || true; fi
  if [[ -n $WORK ]]; then rm -rf "$WORK"; fi
}
trap cleanup EXIT

# finish STATUS REASON [EXIT]: writes metrics.json (python3 -I), keeps serial log, exits.
finish() {
  local s=$1 r=$2 rc=0
  if [[ -n $SERIAL && -f $SERIAL ]]; then cp "$SERIAL" "$OUT_DIR/boot-serial.log" || true; fi
  STATUS=$s REASON=$r OUT=$OUT ISO_PATH=${ISO:-} SERIAL=$SERIAL SCREENSHOT=$([[ -f $SHOT ]] && echo "$SHOT" || true) \
    FW_MODE=$MODE KVM=$KVM WALL_SECONDS=$WALL LOGIN_SEEN=$LOGIN_SEEN \
    python3 -I - <<'PY' || rc=$?
import datetime, hashlib, json, os, re
env = os.environ
def val(k, d=""): return env.get(k, d)
text = ""
if val("SERIAL") and os.path.isfile(val("SERIAL")):
    with open(val("SERIAL"), errors="replace") as f:
        text = f.read()
probe = {}
m = re.search(r"FLAME_PROBE_BEGIN\r?\n(.*?)FLAME_PROBE_END", text, re.S)
if m:
    for line in m.group(1).splitlines():
        k, sep, v = line.strip().partition("=")
        if sep and re.fullmatch(r"[a-z_]+", k):
            probe[k] = v
def num(k, cast=int):
    try:
        return cast(probe[k])
    except (KeyError, ValueError):
        return None
status, reason = val("STATUS", "FAILED"), val("REASON")
if status == "PASS" and (probe.get("lightdm_active") != "1" or num("boot_seconds", float) is None):
    status, reason = "FAILED", reason or "probe incomplete or lightdm not active"
iso, size_mb, sha = val("ISO_PATH"), None, None
if iso and os.path.isfile(iso):
    h = hashlib.sha256()
    with open(iso, "rb") as f:
        size = os.fstat(f.fileno()).st_size
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    sha, size_mb = h.hexdigest(), round(size / 1048576, 1)
top = []
for item in filter(None, probe.get("top_rss", "").split(",")):
    name, _, mb = item.rpartition(":")
    if mb.isdigit():
        top.append({"name": name, "rss_mb": int(mb)})
wall = val("WALL_SECONDS")
doc = {
    "schema": 1,
    "status": status,
    "reason": reason,
    "generated_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "iso": {"path": os.path.basename(iso) if iso else None, "size_mb": size_mb, "sha256": sha},
    "boot": {
        "mode": val("FW_MODE") or None,
        "kvm": val("KVM") == "1",
        "wall_seconds": float(wall) if wall else None,
        "guest_uptime_seconds": num("boot_seconds", float),
        "login_prompt_seen": val("LOGIN_SEEN") == "1",
        "lightdm_active": probe.get("lightdm_active") == "1",
    },
    "systemd": {"state": probe.get("systemd_state"), "analyze": probe.get("systemd_analyze")},
    "memory": {"total_mb": num("mem_total_mb"), "used_mb": num("mem_used_mb"), "available_mb": num("mem_available_mb")},
    "services": {"running": num("services_running")},
    "top_rss": top,
    "screenshot": os.path.basename(val("SCREENSHOT")) if val("SCREENSHOT") else None,
}
with open(val("OUT"), "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
print(status, reason)
raise SystemExit(0 if status == "PASS" else 1)
PY
  exit "${3:-$rc}"
}

command -v python3 >/dev/null || { echo "python3 missing" >&2; exit 2; }
ISO=${1:-}
if [[ -z $ISO ]]; then shopt -s nullglob; isos=("$OUT_DIR"/FlameDE-*.iso); shopt -u nullglob; if (( ${#isos[@]} )); then ISO=${isos[-1]}; fi; fi
if [[ -z $ISO || ! -f $ISO ]]; then finish FAILED "no ISO (arg or $OUT_DIR/FlameDE-*.iso)" 2; fi
command -v qemu-system-x86_64 >/dev/null || finish FAILED "qemu-system-x86_64 missing" 2

OVMF_CODE="" OVMF_VARS_SRC=""
for pair in \
  /usr/share/OVMF/OVMF_CODE_4M.fd:/usr/share/OVMF/OVMF_VARS_4M.fd \
  /usr/share/OVMF/OVMF_CODE.fd:/usr/share/OVMF/OVMF_VARS.fd \
  /usr/share/edk2/ovmf/OVMF_CODE.fd:/usr/share/edk2/ovmf/OVMF_VARS.fd \
  /usr/share/edk2-ovmf/x64/OVMF_CODE.fd:/usr/share/edk2-ovmf/x64/OVMF_VARS.fd; do
  c=${pair%%:*} v=${pair#*:}
  if [[ -r $c && -r $v ]]; then OVMF_CODE=$c OVMF_VARS_SRC=$v; break; fi
done
case $FW in
  auto) if [[ -n $OVMF_CODE ]]; then MODE=uefi; else MODE=bios; fi ;;
  uefi) [[ -n $OVMF_CODE ]] || finish FAILED "FW=uefi but no OVMF firmware found" 2; MODE=uefi ;;
  bios) MODE=bios ;;
  *) finish FAILED "bad FW=$FW (auto|uefi|bios)" 2 ;;
esac

ACCEL=(-machine q35 -accel tcg) CPU=max
if [[ -r /dev/kvm && -w /dev/kvm ]]; then ACCEL=(-machine q35 -accel kvm) CPU=host KVM=1; fi

WORK=$(mktemp -d)
SERIAL=$WORK/serial.log
QMP=$WORK/qmp.sock
FWARGS=()
if [[ $MODE == uefi ]]; then
  cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
  FWARGS=(-drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE" -drive "if=pflash,format=raw,file=$WORK/vars.fd")
fi

START=$(date +%s.%N)
qemu-system-x86_64 "${ACCEL[@]}" -cpu "$CPU" -smp "$SMP" -m "$MEM" \
  -display none -monitor none -no-reboot -rtc base=utc \
  -cdrom "$ISO" "${FWARGS[@]}" \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
  -serial "file:$SERIAL" -qmp "unix:$QMP,server=on,wait=off" \
  >"$WORK/qemu.out" 2>&1 &
QPID=$!

deadline=$((SECONDS + BOOT_TIMEOUT))
while :; do
  if [[ -f $SERIAL ]]; then
    if [[ -z $WALL ]] && grep -aq 'FLAME_LIGHTDM_ACTIVE' "$SERIAL"; then
      WALL=$(awk -v s="$START" -v e="$(date +%s.%N)" 'BEGIN{printf "%.2f", e - s}')
    fi
    if [[ $LOGIN_SEEN == 0 ]] && grep -aq ' login:' "$SERIAL"; then LOGIN_SEEN=1; fi
    if grep -aq 'FLAME_PROBE_END' "$SERIAL"; then break; fi
  fi
  if ! kill -0 "$QPID" 2>/dev/null; then
    QPID=""
    finish FAILED "qemu exited before FLAME_PROBE_END; see $WORK/qemu.out" 1
  fi
  if (( SECONDS >= deadline )); then
    finish FAILED "timeout ${BOOT_TIMEOUT}s waiting for FLAME_PROBE_END" 1
  fi
  sleep 2
done

# Screenshot via QMP while the guest is still running (best effort).
python3 -I - "$QMP" "$SHOT" <<'PY' || echo "screendump failed (non-fatal)" >&2
import json, socket, sys
path, out = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX)
s.settimeout(10)
s.connect(path)
f = s.makefile("rwb")
f.readline()
def call(cmd, args=None):
    msg = {"execute": cmd}
    if args:
        msg["arguments"] = args
    f.write((json.dumps(msg) + "\n").encode())
    f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            return r
call("qmp_capabilities")
r = call("screendump", {"filename": out})
if "error" in r:
    raise SystemExit(r["error"])
PY

finish PASS ""
