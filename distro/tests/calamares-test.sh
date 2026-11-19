#!/usr/bin/env bash
# Calamares GUI install test for the FlameDE release ISO, then installed-disk boot checks.
#
# Phases (fail-closed: any missing prerequisite or failed check exits non-zero):
#   1. prereqs   qemu-system-x86_64, qemu-img, isoinfo, python3, OVMF, PIL, the ISO.
#   2. live      Boot the ISO (kernel/initrd from the ISO, console=ttyS0), blank qcow2 target,
#                serial login as flamelive (no password). Check /usr/bin/unsquashfs and the
#                installed "Install FlameDE" launcher exist. Nothing is uploaded into the guest.
#   3. install   Start the installer by running the launcher's Exec line verbatim
#                (sh -c "sudo -E calamares") with DISPLAY/XAUTHORITY set for the lightdm X
#                session. Drive the GUI with QMP (clicks/keys). Poll the guest session.log until
#                the install reaches "Unmounting file systems" 33/33 with no "Installation failed".
#                Copy session.log to distro/out/calamares/session.log.
#   4. disk      Test-only GRUB edit on the target (console=ttyS0 appended to grub.cfg, harness
#                convention, not a product change). Power the live VM off, boot the target alone
#                (fresh OVMF NVRAM), serial login as tester, and check: is-system-running,
#                lightdm active, flamelive removed, root on /dev/vda2, no autologin-* keys.
#
# Exit codes: 0 = all checks PASS; 1 = a check FAILED; 2 = prerequisite/harness error (BLOCKED).
# Outputs: distro/out/calamares/ (session.log, screenshots, disk-phase-results.txt, qemu stderr).
# Passwords (flametest-1234, tester, flametest) are throwaway test values for the installer
# form and appear in logs by design, as in install-test.sh.
#
# Usage: distro/tests/calamares-test.sh [path/to/FlameDE-*.iso]
#        Default ISO: distro/out/FlameDE-0.1.7-amd64.iso. Environment: WORK (default
#        $TMPDIR/calamares-test-XXXX), OVMF_CODE, OVMF_VARS_TEMPLATE, MEM (default 4096).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ISO="${1:-$REPO/distro/out/FlameDE-0.1.7-amd64.iso}"
OUT="$REPO/distro/out/calamares"
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
OVMF_VARS_TEMPLATE="${OVMF_VARS_TEMPLATE:-/usr/share/OVMF/OVMF_VARS_4M.fd}"
MEM="${MEM:-4096}"
DISK_GB="${DISK_GB:-24}"

die_blocked() { echo "BLOCKED: $*" >&2; exit 2; }

for t in qemu-system-x86_64 qemu-img isoinfo python3; do
    command -v "$t" >/dev/null 2>&1 || die_blocked "missing tool: $t"
done
[ -r "$ISO" ] || die_blocked "ISO not readable: $ISO"
[ -r "$OVMF_CODE" ] || die_blocked "OVMF code not found: $OVMF_CODE"
[ -r "$OVMF_VARS_TEMPLATE" ] || die_blocked "OVMF vars template not found: $OVMF_VARS_TEMPLATE"
python3 -I -c 'import PIL' 2>/dev/null || die_blocked "python3 PIL module required for screenshots"

WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/calamares-test-XXXX")}"
mkdir -p "$OUT"
SOCK="$WORK/sock"
mkdir -p "$SOCK"

isoinfo -i "$ISO" -R -x /casper/vmlinuz > "$WORK/vmlinuz" || die_blocked "cannot extract casper/vmlinuz"
isoinfo -i "$ISO" -R -x /casper/initrd > "$WORK/initrd" || die_blocked "cannot extract casper/initrd"
qemu-img create -q -f qcow2 "$WORK/target.qcow2" "${DISK_GB}G"
cp "$OVMF_VARS_TEMPLATE" "$WORK/live-vars.fd"
cp "$OVMF_VARS_TEMPLATE" "$WORK/disk-vars.fd"

# Harness helper (stdlib only, python3 -I). Subcommands:
#   serial <cmd> <timeout>          run a shell command on the guest serial console, print output
#   login <user> <password|->      log in on ttyS0 (empty password = no prompt)
#   click X Y | key NAME | shot NAME | wake  (QMP input and screendump)
#   gui                             drive the installer from the Welcome page to Install
#   wait-install <timeout>          poll session.log until Finish (33/33) or failure
#   fetch <guest-path> <local>      copy a root-readable guest file over serial (base64)
HELPER="$WORK/cal.py"
cat > "$HELPER" <<'PY'
import base64, json, os, re, socket, sys, time
SK = os.environ["CAL_SOCK"]
OUT = os.environ["CAL_OUT"]
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[()][0-9A-Za-z]|\x1b[=>78]")
QCODE = {c: c for c in "abcdefghijklmnopqrstuvwxyz0123456789"}
QCODE.update({"-": "minus", ".": "dot", "/": "slash", " ": "spc", "_": "minus", "\n": "ret"})

def sock(path):
    s = socket.socket(socket.AF_UNIX); s.connect(path); s.settimeout(0.5); return s

def drain(s, secs):
    buf, end = b"", time.time() + secs
    while time.time() < end:
        try:
            d = s.recv(1 << 20)
        except socket.timeout:
            continue
        if not d:
            break
        buf += d
    return ANSI.sub("", buf.decode("utf-8", "replace")).replace("\r", "")

def ser(cmd, timeout=120):
    s = sock(f"{SK}/serial.sock")
    tag = str(int(time.time() * 1000) % 100000)
    s.sendall(f'{cmd}; echo "FL""DONE{tag}:$?"\n'.encode())
    buf, end = "", time.time() + timeout
    while time.time() < end:
        try:
            d = s.recv(1 << 20)
        except socket.timeout:
            continue
        if not d:
            break
        buf += ANSI.sub("", d.decode("utf-8", "replace")).replace("\r", "")
        m = re.search(rf"FL ?DONE{tag}:(\d+)", buf)
        if m:
            s.close()
            return int(m.group(1)), buf[: m.start()]
    s.close()
    return -1, buf

def qmp(cmd, **args):
    s = sock(f"{SK}/qmp.sock")
    f = s.makefile("rw")
    f.readline()
    f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n"); f.flush(); f.readline()
    f.write(json.dumps({"execute": cmd, "arguments": args}) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            s.close()
            return r

def click(x, y, w=1280, h=800):
    ax, ay = int(x * 32767 / w), int(y * 32767 / h)
    ev = lambda b: {"events": [{"type": "abs", "data": {"axis": "x", "value": ax}},
                               {"type": "abs", "data": {"axis": "y", "value": ay}},
                               {"type": "btn", "data": {"down": b, "button": "left"}}]}
    qmp("input-send-event", **ev(True)); time.sleep(0.15); qmp("input-send-event", **ev(False))

def key(name):
    qmp("send-key", keys=[{"type": "qcode", "data": name}], **{"hold-time": 60})

def typed(text):
    for ch in text:
        if ch.isupper():
            qmp("send-key", keys=[{"type": "qcode", "data": "shift"}, {"type": "qcode", "data": ch.lower()}], **{"hold-time": 60})
        else:
            q = QCODE.get(ch)
            if q is None:
                raise SystemExit(f"no qcode for {ch!r}")
            qmp("send-key", keys=[{"type": "qcode", "data": q}], **{"hold-time": 60})
        time.sleep(0.05)

def shot(name):
    from PIL import Image
    ppm = os.path.join(OUT, f"{name}.ppm")
    qmp("screendump", filename=ppm)
    png = os.path.join(OUT, f"{name}.png")
    Image.open(ppm).convert("RGB").save(png); os.remove(ppm)
    print(png)

def login(user, pw):
    s = sock(f"{SK}/serial.sock")
    s.sendall(b"\x03\n"); time.sleep(2); drain(s, 1)
    s.sendall(user.encode() + b"\n"); time.sleep(3); drain(s, 1)
    if pw != "-":
        s.sendall(pw.encode() + b"\n")
    time.sleep(5)
    out = drain(s, 1)
    s.close()
    ok = ("$ " in out) or ("#" in out[-40:])
    print("LOGIN_OK" if ok else "LOGIN_FAIL")

def gui():
    # Coordinates are for the 1280x800 installer window (same layout as the approved harness run).
    NEXT, INSTALL = (1155, 731), (1152, 731)
    def clr(x, y):
        click(x, y); time.sleep(0.6); key("end"); time.sleep(0.2)
        for _ in range(25):
            key("backspace"); time.sleep(0.08)
    def typ(s):
        typed(s); time.sleep(0.5)
    click(730, 441); time.sleep(0.3); click(730, 441); time.sleep(3)   # maximize
    click(640, 780); time.sleep(1)
    for _ in range(4):
        click(*NEXT); time.sleep(3)
    click(300, 83); typ("Tester Flame")
    clr(300, 143); typ("tester")
    clr(300, 203); typ("flametest")
    clr(300, 263); typ("flametest-1234")
    clr(505, 263); typ("flametest-1234")
    time.sleep(1); click(*NEXT); time.sleep(3)
    click(*INSTALL); time.sleep(3)
    print("GUI_STARTED")

def wait_install(timeout):
    end = time.time() + timeout
    while time.time() < end:
        rc, out = ser("grep -c 'Installation failed' /home/flamelive/.cache/calamares/session.log; grep -E 'Starting job .*\\( 33 / 33 \\)' /home/flamelive/.cache/calamares/session.log | tail -n 1", 60)
        if "Installation failed" in out and not re.search(r"^0$", out, re.M):
            print("INSTALL_FAILED"); return
        if "33 / 33" in out:
            time.sleep(20)
            print("INSTALL_DONE"); return
        click(640, 700); time.sleep(40)  # keep the display awake (DPMS blanks mid-install)
    print("INSTALL_TIMEOUT")

def fetch(remote, local):
    rc, out = ser(f"sudo -n base64 -w0 {remote}; echo", 240)
    m = re.search(r"([A-Za-z0-9+/=]{64,})", out)
    if not m:
        print("FETCH_FAIL"); return
    open(local, "wb").write(base64.b64decode(m.group(1)))
    print("FETCH_OK", os.path.getsize(local))

cmd, args = sys.argv[1], sys.argv[2:]
if cmd == "serial":
    rc, out = ser(args[0], int(args[1]) if len(args) > 1 else 120)
    print("RC", rc); print(out[-4000:])
elif cmd == "login":
    login(args[0], args[1])
elif cmd == "click":
    click(int(args[0]), int(args[1]))
elif cmd == "key":
    key(args[0])
elif cmd == "shot":
    shot(args[0])
elif cmd == "wake":
    click(640, 700)
elif cmd == "gui":
    gui()
elif cmd == "wait-install":
    wait_install(int(args[0]))
elif cmd == "fetch":
    fetch(args[0], args[1])
else:
    raise SystemExit(f"unknown command {cmd}")
PY

export CAL_SOCK="$SOCK" CAL_OUT="$OUT"
H() { python3 -I "$HELPER" "$@"; }

start_vm() {  # $1 = "live" (ISO attached) or "disk"
    local common=(-machine q35 -accel kvm -cpu host -smp 2 -m "$MEM" -vga std
        -device qemu-xhci -device usb-tablet -display none -monitor none -no-reboot -rtc base=utc
        -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE")
    rm -f "$SOCK/serial.sock" "$SOCK/qmp.sock"
    if [ "$1" = live ]; then
        qemu-system-x86_64 "${common[@]}" -drive "if=pflash,format=raw,file=$WORK/live-vars.fd" \
            -cdrom "$ISO" -kernel "$WORK/vmlinuz" -initrd "$WORK/initrd" \
            -append "boot=casper console=ttyS0 ---" \
            -drive "file=$WORK/target.qcow2,format=qcow2,if=virtio" \
            -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
            -chardev "socket,id=ser0,path=$SOCK/serial.sock,server=on,wait=off" -serial chardev:ser0 \
            -qmp "unix:$SOCK/qmp.sock,server=on,wait=off" >"$WORK/live-qemu.err" 2>&1 &
    else
        qemu-system-x86_64 "${common[@]}" -drive "if=pflash,format=raw,file=$WORK/disk-vars.fd" \
            -drive "file=$WORK/target.qcow2,format=qcow2,if=virtio" \
            -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
            -chardev "socket,id=ser0,path=$SOCK/serial.sock,server=on,wait=off" -serial chardev:ser0 \
            -qmp "unix:$SOCK/qmp.sock,server=on,wait=off" >"$WORK/disk-qemu.err" 2>&1 &
    fi
    VM_PID=$!
    sleep 5
}

stop_vm() {
    if [ -n "${VM_PID:-}" ] && kill -0 "$VM_PID" 2>/dev/null; then
        kill -9 "$VM_PID" 2>/dev/null || true
        wait "$VM_PID" 2>/dev/null || true
    fi
    VM_PID=
}
trap stop_vm EXIT

RESULTS="$OUT/disk-phase-results.txt"
: > "$RESULTS"
FAILS=0
check() {  # $1 = name, $2 = "PASS"|"FAIL", $3 = evidence
    echo "$2 $1: $3" | tee -a "$RESULTS"
    [ "$2" = PASS ] || FAILS=$((FAILS + 1))
}

# Phase 2: live boot and prerequisites
start_vm live
H serial 'echo ready' 1 >/dev/null 2>&1 || true
for _ in $(seq 1 12); do
    if H login flamelive - | grep -q LOGIN_OK; then break; fi
    sleep 15
done
H login flamelive - | grep -q LOGIN_OK || die_blocked "live login failed (serial)"

rc=$(H serial 'test -x /usr/bin/unsquashfs && echo UNSQ_OK; test -f /usr/share/applications/flamede-install.desktop && echo DESK_OK' 60)
EXEC_LINE="$(H serial "grep -m1 '^Exec=' /usr/share/applications/flamede-install.desktop" 60 | grep -o 'Exec=.*' | head -n1)"
case "$rc" in *UNSQ_OK*) check "image unsquashfs" PASS "/usr/bin/unsquashfs present" ;; *) check "image unsquashfs" FAIL "/usr/bin/unsquashfs missing" ;; esac
case "$rc" in *DESK_OK*) check "launcher present" PASS "flamede-install.desktop installed" ;; *) check "launcher present" FAIL "flamede-install.desktop missing" ;; esac
[ -n "$EXEC_LINE" ] || die_blocked "launcher has no Exec line"

# Phase 3: start the installer from the launcher Exec line and drive the GUI
EXEC_CMD="${EXEC_LINE#Exec=}"
H serial "DISPLAY=:0 XAUTHORITY=/var/run/lightdm/root/:0 nohup $EXEC_CMD >/tmp/launch.log 2>&1 & echo LAUNCHED" 60 >/dev/null
sleep 25
H gui | tee -a "$WORK/gui.log"
H shot 01-installer-maximized >/dev/null 2>&1 || true
H wait-install 1800 | tee -a "$WORK/install.log"
H shot 05-finish >/dev/null 2>&1 || true
if grep -q INSTALL_DONE "$WORK/install.log"; then check "calamares install" PASS "reached 33/33, no Installation failed"
else check "calamares install" FAIL "$(tail -n1 "$WORK/install.log")"; fi

H fetch /home/flamelive/.cache/calamares/session.log "$OUT/session.log" | tee -a "$WORK/install.log"
grep -q "Installation failed" "$OUT/session.log" 2>/dev/null && check "session.log has no failure" FAIL "Installation failed in log" || check "session.log has no failure" PASS "no Installation failed"

# Phase 4: test-only GRUB edit, power off live, boot the target alone
H serial 'sudo -n mkdir -p /mnt/t; sudo -n mount /dev/vda2 /mnt/t && sudo -n sed -i "s/quiet splash/console=ttyS0 console=tty1 quiet/" /mnt/t/boot/grub/grub.cfg; sudo -n umount /mnt/t; sync; echo GRUB_EDIT_DONE' 240 >/dev/null
stop_vm
start_vm disk
for _ in $(seq 1 12); do
    if H serial 'echo ready' 20 | grep -q 'login:\|ready'; then break; fi
    sleep 10
done
H login tester flametest-1234 | grep -q LOGIN_OK && check "disk login" PASS "tester on serial console" || check "disk login" FAIL "tester login failed"

ISO_CHECK="$(H serial 'echo SYS_$(systemctl is-system-running)_LW_$(systemctl is-active lightdm)_ROOT_$(findmnt -no SOURCE /)_FL_$(id flamelive >/dev/null 2>&1 && echo present || echo absent)_AL_$(grep -l "^autologin-" /etc/lightdm/lightdm.conf.d/*.conf 2>/dev/null | wc -l)_X' 120)"
case "$ISO_CHECK" in *SYS_running_LW_active*) check "systemctl is-system-running" PASS "running"; check "lightdm active" PASS "active" ;; *) check "systemctl is-system-running" FAIL "$ISO_CHECK" ;; esac
case "$ISO_CHECK" in *ROOT_/dev/vda2*) check "root on target" PASS "/dev/vda2" ;; *) check "root on target" FAIL "root not /dev/vda2" ;; esac
case "$ISO_CHECK" in *FL_absent*) check "flamelive removed" PASS "no such user" ;; *) check "flamelive removed" FAIL "flamelive still present" ;; esac
case "$ISO_CHECK" in *AL_0*) check "no live autologin" PASS "no autologin-* keys" ;; *) check "no live autologin" FAIL "autologin keys remain" ;; esac

H shot 06-disk-booted >/dev/null 2>&1 || true
stop_vm

echo "FAILS=$FAILS" | tee -a "$RESULTS"
[ "$FAILS" -eq 0 ] && exit 0
exit 1
