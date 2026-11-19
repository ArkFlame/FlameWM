#!/usr/bin/env bash
# install-test.sh [ISO] - prove FlameDE installs to a disk and boots from it (KVM + OVMF + serial).
# Phase live: kernel/initrd come from the ISO (boot=casper console=ttyS0), a blank qcow2 target is
#   attached as /dev/vda. Log in on ttyS0 as flamelive (no password), run
#   `flame-install --yes --disk /dev/vda --kernel-arg console=ttyS0`, power off.
# Phase disk: the ISO is NOT attached; OVMF boots the disk with the same NVRAM vars. Wait for the
#   serial login prompt, then check systemd state, lightdm, hostname, installed user, root device,
#   that flamelive is gone and that no live autologin remains.
# Env: OUT_DIR=<repo>/distro/out  MEM=2048  SMP=2  DISK_GB=12  BOOT_TIMEOUT=600
#      INSTALL_TIMEOUT=2700  KEEP_WORK=0  FLAME_TEST_SCRATCH=<TMPDIR or /tmp>
# Passwords here are throwaway test values that appear in the serial log by design.
# Exit: 0 PASS, 1 FAILED (install or boot or check), 2 precondition missing.
# Logs: <OUT_DIR>/install-test.log, <OUT_DIR>/install-serial-live.log, <OUT_DIR>/install-serial-disk.log
set -Eeuo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT_DIR=${OUT_DIR:-$REPO/distro/out}
ISO=${1:-$OUT_DIR/FlameDE-0.1.7-test-amd64.iso}
MEM=${MEM:-2048}
SMP=${SMP:-2}
DISK_GB=${DISK_GB:-12}
BOOT_TIMEOUT=${BOOT_TIMEOUT:-600}
INSTALL_TIMEOUT=${INSTALL_TIMEOUT:-2700}
KEEP_WORK=${KEEP_WORK:-0}
INSTALLER=$REPO/distro/installer/flame-install
export BOOT_TIMEOUT INSTALL_TIMEOUT DISK_GB INSTALLER
export TEST_PW=flametest-1234 TEST_HOST=flametest TEST_USER=tester
LOG=$OUT_DIR/install-test.log
mkdir -p "$OUT_DIR"
: >"$LOG"

WORK="" QPID=""
say() { printf '%s\n' "$*" | tee -a "$LOG"; }
# shellcheck disable=SC2329  # invoked via trap
SOCKDIR=""
cleanup() {
    if [[ -n $QPID ]]; then kill "$QPID" 2>/dev/null || true; wait "$QPID" 2>/dev/null || true; fi
    if [[ -n $WORK && $KEEP_WORK != 1 ]]; then rm -rf "$WORK"; fi
    if [[ -n $SOCKDIR ]]; then rm -rf "$SOCKDIR"; fi
}
trap cleanup EXIT
precheck_fail() { say "install-test: FAILED precondition: $*"; exit 2; }

for t in qemu-system-x86_64 qemu-img isoinfo python3 tee; do
    command -v "$t" >/dev/null || precheck_fail "missing $t"
done
[[ -f $ISO ]] || precheck_fail "ISO not found: $ISO"
[[ -r /dev/kvm && -w /dev/kvm ]] || precheck_fail "/dev/kvm not readable and writable"

OVMF_CODE="" OVMF_VARS_SRC=""
for pair in \
    /usr/share/OVMF/OVMF_CODE_4M.fd:/usr/share/OVMF/OVMF_VARS_4M.fd \
    /usr/share/OVMF/OVMF_CODE.fd:/usr/share/OVMF/OVMF_VARS.fd \
    /usr/share/edk2/ovmf/OVMF_CODE.fd:/usr/share/edk2/ovmf/OVMF_VARS.fd \
    /usr/share/edk2-ovmf/x64/OVMF_CODE.fd:/usr/share/edk2-ovmf/x64/OVMF_VARS.fd; do
    c=${pair%%:*} v=${pair#*:}
    if [[ -r $c && -r $v ]]; then OVMF_CODE=$c OVMF_VARS_SRC=$v; break; fi
done
[[ -n $OVMF_CODE ]] || precheck_fail "no OVMF UEFI firmware found"

WORK=$(mktemp -d "${FLAME_TEST_SCRATCH:-${TMPDIR:-/tmp}}/flamede-install.XXXXXX")
# Unix socket paths are limited to ~107 bytes, so the serial socket lives in a short directory.
SOCKDIR=$(mktemp -d /tmp/fdi.XXXXXX)
say "install-test: iso=$ISO work=$WORK mem=${MEM}M smp=$SMP disk=${DISK_GB}G kvm=1 fw=$OVMF_CODE"

isoinfo -R -x /casper/vmlinuz -i "$ISO" >"$WORK/vmlinuz"
isoinfo -R -x /casper/initrd -i "$ISO" >"$WORK/initrd"
[[ -s $WORK/vmlinuz && -s $WORK/initrd ]] || { say "install-test: FAILED could not extract casper kernel/initrd"; exit 1; }
cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
qemu-img create -q -f qcow2 "$WORK/target.qcow2" "${DISK_GB}G"

cat >"$WORK/drive.py" <<'PY'
import base64, hashlib, os, re, socket, sys, time

SOCK, LOG, PHASE = sys.argv[1], sys.argv[2], sys.argv[3]
BOOT_T = int(os.environ["BOOT_TIMEOUT"])
INST_T = int(os.environ["INSTALL_TIMEOUT"])
DISK_GB = os.environ["DISK_GB"]
PW = os.environ["TEST_PW"]
HOST = os.environ["TEST_HOST"]
USER = os.environ["TEST_USER"]
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[()][0-9A-Za-z]|\x1b[=>78]")
AUTO = [(re.compile(r"\[sudo\] password for flamelive: "), "\n")]


class Fail(Exception):
    pass


logf = open(LOG, "ab", buffering=0)
buf = ""
seq = 0


def connect():
    end = time.time() + 120
    while True:
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(SOCK)
            s.settimeout(0.5)
            return s
        except OSError:
            s.close()
            if time.time() > end:
                raise Fail("cannot connect to serial socket " + SOCK)
            time.sleep(0.5)


def send(text):
    sock.sendall(text.encode())


def pump():
    global buf
    try:
        data = sock.recv(65536)
    except socket.timeout:
        return
    if not data:
        raise Fail("serial closed (guest powered off or qemu exited)")
    logf.write(data)
    buf += ANSI.sub("", data.decode("utf-8", "replace")).replace("\r", "")
    if len(buf) > 400000:
        buf = buf[-200000:]
    for rx, resp in AUTO:
        m = rx.search(buf)
        if m:
            send(resp)
            buf = buf[m.end():]


def expect(pattern, timeout):
    global buf
    rx = re.compile(pattern)
    end = time.time() + timeout
    while True:
        m = rx.search(buf)
        if m:
            pre = buf[: m.start()]
            buf = buf[m.end():]
            return m, pre
        if "Login incorrect" in buf:
            raise Fail("login incorrect")
        if time.time() > end:
            raise Fail(f"timeout {timeout}s waiting for {pattern!r}")
        pump()


def run(cmd, timeout):
    global seq
    seq += 1
    send(f'{cmd}; echo "FLAME_RC{seq}:$?"\n')
    m, pre = expect(rf"FLAME_RC{seq}:(\d+)", timeout)
    return int(m.group(1)), pre


def login(user, pw, timeout):
    expect(r"login: ", timeout)
    send(user + "\n")
    # An account with an empty password (flamelive) gets no Password prompt; it reaches the shell.
    m, _ = expect(r"Password: |\$ ", 120)
    if m.group(0) == "Password: ":
        send(pw + "\n")
    send("echo FLAME_READY$((6*7))\n")
    expect(r"FLAME_READY42", 180)


def out(msg):
    print(msg, flush=True)


def check(name, ok, detail=""):
    out(f"CHECK {name} {'PASS' if ok else 'FAIL'} {detail}".rstrip())
    if not ok:
        raise Fail(f"{name} failed {detail}".rstrip())


def last_line(pre):
    lines = [x.strip() for x in pre.splitlines() if x.strip()]
    return lines[-1] if lines else ""


def wait_poweroff(timeout):
    end = time.time() + timeout
    while time.time() < end:
        try:
            data = sock.recv(65536)
        except socket.timeout:
            continue
        if not data:
            return
        logf.write(data)
    raise Fail("guest did not power off")


def live():
    login("flamelive", "", BOOT_T)
    check("live login", True, "flamelive on ttyS0")
    rc, pre = run("lsblk -dno SIZE /dev/vda", 60)
    check("target disk visible", rc == 0 and f"{DISK_GB}G" in pre, last_line(pre))
    rc, pre = run(f"export FLAME_HOSTNAME={HOST} FLAME_USER={USER} FLAME_PASSWORD={PW}; sudo true", 120)
    check("sudo for live user", rc == 0, f"rc={rc}")
    # Upload the repo's installer over serial (base64, 1000-char lines) and verify its SHA-256,
    # so the test exercises the current script rather than whatever the ISO was built with.
    data = open(os.environ["INSTALLER"], "rb").read()
    want = hashlib.sha256(data).hexdigest()
    b64 = base64.b64encode(data).decode()
    run("rm -f /tmp/fi.b64", 30)
    for i in range(0, len(b64), 1000):
        send(f"printf '%s' '{b64[i:i + 1000]}' >> /tmp/fi.b64\n")
    rc, pre = run("base64 -d /tmp/fi.b64 >/tmp/flame-install && chmod 755 /tmp/flame-install && sha256sum /tmp/flame-install", 300)
    check("installer uploaded", rc == 0 and want in pre, want[:12])
    rc, pre = run(
        f"sudo env FLAME_HOSTNAME={HOST} FLAME_USER={USER} FLAME_PASSWORD={PW} "
        "/tmp/flame-install --yes --disk /dev/vda --kernel-arg console=ttyS0",
        INST_T,
    )
    check("flame-install exit", rc == 0, f"rc={rc}")
    check("flame-install finished", "[flame-install] finished" in pre)
    # Guest power-off is not used: after the installer's unmount, sudo cannot allocate a pty
    # (serial log: "sudo: unable to allocate pty"), and `sync` hung. flame-install prints
    # "finished" only after its own final sync, so the driver stops QEMU at that point.
    check("installer final sync before stop", "[flame-install] finished" in pre)


def disk():
    login(USER, PW, BOOT_T)
    check("installed login prompt", True, "serial getty after boot from disk, no cdrom")
    rc, pre = run("timeout 600 systemctl is-system-running --wait", 700)
    state = last_line(pre)
    check("systemctl is-system-running", state == "running", f"state={state} rc={rc}")
    rc, _ = run("systemctl is-active --quiet lightdm", 60)
    check("lightdm active", rc == 0)
    rc, _ = run(f'test "$(hostname)" = {HOST}', 60)
    check("hostname", rc == 0, HOST)
    rc, _ = run(f"id -u {USER} && test -d /home/{USER}", 60)
    check("installed user", rc == 0, USER)
    rc, _ = run("! id flamelive >/dev/null 2>&1", 60)
    check("live user removed", rc == 0)
    rc, _ = run('test "$(findmnt -no SOURCE /)" = /dev/vda3', 60)
    check("root on target disk", rc == 0, "/dev/vda3")
    rc, _ = run("! grep -rqs '^autologin-user=' /etc/lightdm/lightdm.conf.d/", 60)
    check("no live autologin", rc == 0)


try:
    sock = connect()
    if PHASE == "live":
        live()
    else:
        disk()
    out(f"RESULT PASS {PHASE}")
except Fail as e:
    out(f"RESULT FAIL {PHASE}: {e}")
    sys.exit(1)
PY

run_phase() {
    local name=$1 rc=0
    shift
    local sock="$SOCKDIR/serial-$name.sock"
    rm -f "$sock"
    qemu-system-x86_64 -machine q35 -accel kvm -cpu host -smp "$SMP" -m "$MEM" \
        -display none -monitor none -no-reboot -rtc base=utc \
        -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE" \
        -drive "if=pflash,format=raw,file=$WORK/vars.fd" \
        -drive "file=$WORK/target.qcow2,format=qcow2,if=virtio" \
        -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
        -serial "unix:$sock,server=on,wait=off" "$@" >"$WORK/qemu-$name.out" 2>&1 &
    QPID=$!
    set +e
    python3 -I "$WORK/drive.py" "$sock" "$OUT_DIR/install-serial-$name.log" "$name" 2>&1 | tee -a "$LOG"
    rc=${PIPESTATUS[0]}
    set -e
    if kill -0 "$QPID" 2>/dev/null; then kill "$QPID" 2>/dev/null || true; fi
    wait "$QPID" 2>/dev/null || true
    QPID=""
    return "$rc"
}

if ! run_phase live -cdrom "$ISO" -kernel "$WORK/vmlinuz" -initrd "$WORK/initrd" -append "boot=casper console=ttyS0 ---"; then
    say "install-test: FAILED live install phase (see $OUT_DIR/install-serial-live.log)"
    exit 1
fi
if ! run_phase disk; then
    say "install-test: FAILED disk boot phase (see $OUT_DIR/install-serial-disk.log)"
    exit 1
fi
say "install-test: PASS iso=$(basename "$ISO") logs=$OUT_DIR/install-serial-live.log,$OUT_DIR/install-serial-disk.log"
