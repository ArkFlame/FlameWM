#!/usr/bin/env python3
"""X11 CLIPBOARD/PRIMARY selection acceptance probe.

Starts a private Xvfb on a free display in :90..:99 with isolated HOME and
XDG_RUNTIME_DIR, then runs a python-xlib selection owner (serves TARGETS,
UTF8_STRING, STRING, text/plain;charset=utf-8 and INCR for payloads above
200000 bytes), a requestor that converts and reassembles (including INCR),
and a third client that takes CLIPBOARD to verify SelectionClear delivery.

Phase no_wm always runs. Phase with_wm runs only when --with-wm WM_BIN names
an existing executable; the WM is started on the same display first.
Payload bytes are never printed; only sha256 digests and lengths are reported.

Exit codes: 0 no FAIL, 1 at least one FAIL, 3 environment blocked (no Xvfb
or no free display).
"""
import argparse
import hashlib
import json
import os
import random
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import queue

try:
    import Xlib  # noqa: F401
except ImportError:
    # python3 -I skips the user site directory, where python-xlib is installed here.
    import site
    site.addsitedir(site.getusersitepackages())

from Xlib import X, Xatom  # noqa: E402
from Xlib import display as xdisplay  # noqa: E402
from Xlib import error as xerror  # noqa: E402
from Xlib.protocol import event as xevent  # noqa: E402

from flamewm_harness_common import display_in_use, stop_process  # noqa: E402

DISPLAY_RANGE = range(90, 100)
INCR_THRESHOLD = 200000
INCR_CHUNK = 65536
TEXT_TARGETS = ("UTF8_STRING", "STRING", "text/plain;charset=utf-8")
TIMEOUT = 10.0
EXIT_OK, EXIT_FAIL, EXIT_ENV = 0, 1, 3


def sha(data):
    return hashlib.sha256(data).hexdigest()


def make_payloads():
    rng = random.Random(7)
    return {
        "ascii": b"flamewm clipboard probe",
        "multiline": "a\nb\n".encode("utf-8"),
        "unicode": "é€😀".encode("utf-8"),
        "mib": bytes(rng.choices(b"abcdefghij\n", k=1 << 20)),
    }


def as_window(d, wid):
    return wid if hasattr(wid, "id") else d.create_resource_object("window", wid)


def make_window(d):
    root = d.screen().root
    return root.create_window(0, 0, 1, 1, 0, d.screen().root_depth,
                              event_mask=X.PropertyChangeMask)


class Owner(threading.Thread):
    """Owns CLIPBOARD/PRIMARY on its own display connection."""

    def __init__(self, name):
        super().__init__(daemon=True)
        self.d = xdisplay.Display(name)
        self.win = make_window(self.d)
        self.a = {n: self.d.intern_atom(n) for n in
                  ("CLIPBOARD", "PRIMARY", "TARGETS", "INCR", *TEXT_TARGETS)}
        self.cmds = queue.Queue()
        self.data = {}
        self.clears = []
        self.incr = {}
        self.stop_ev = threading.Event()
        self.error = None

    def own(self, sel, data):
        reply = {"ev": threading.Event(), "ok": False}
        self.cmds.put((sel, data, reply))
        if not reply["ev"].wait(TIMEOUT):
            raise RuntimeError("owner command timeout")
        if not reply["ok"]:
            raise RuntimeError(f"owner did not acquire {sel}")

    def run(self):
        try:
            while not self.stop_ev.is_set():
                while True:
                    try:
                        sel, data, reply = self.cmds.get_nowait()
                    except queue.Empty:
                        break
                    self.data[self.a[sel]] = data
                    self.win.set_selection_owner(self.a[sel], X.CurrentTime)
                    self.d.flush()
                    reply["ok"] = self.d.get_selection_owner(self.a[sel]).id == self.win.id
                    reply["ev"].set()
                self._drain(0.02)
        except Exception as exc:  # reported through owner command timeouts
            self.error = f"{type(exc).__name__}: {exc}"

    def _drain(self, timeout):
        select.select([self.d.fileno()], [], [], timeout)
        while self.d.pending_events():
            self._handle(self.d.next_event())

    def _handle(self, ev):
        if ev.type == X.SelectionClear:
            self.clears.append(self.d.get_atom_name(ev.atom))
            self.data.pop(ev.atom, None)
        elif ev.type == X.SelectionRequest:
            self._serve(ev)
        elif ev.type == X.PropertyNotify and ev.state == X.PropertyDelete:
            key = (ev.window.id, ev.atom)
            if key in self.incr:
                self._incr_step(key)

    def _serve(self, ev):
        req = as_window(self.d, ev.requestor)
        target = self.d.get_atom_name(ev.target)
        prop = ev.property or ev.target
        data = self.data.get(ev.selection)
        ok = False
        if data is not None and target == "TARGETS":
            atoms = [self.a["TARGETS"]] + [self.a[t] for t in TEXT_TARGETS]
            req.change_property(prop, Xatom.ATOM, 32, atoms)
            ok = True
        elif data is not None and target in TEXT_TARGETS:
            if len(data) > INCR_THRESHOLD:
                req.change_property(prop, self.a["INCR"], 32, [len(data)])
                req.change_attributes(event_mask=X.PropertyChangeMask)
                self.incr[(req.id, prop)] = {"data": data, "off": 0,
                                             "target": ev.target, "win": req}
            else:
                req.change_property(prop, ev.target, 8, data)
            ok = True
        note = xevent.SelectionNotify(
            time=ev.time, requestor=req.id, selection=ev.selection,
            target=ev.target, property=prop if ok else X.NONE)
        req.send_event(note, event_mask=0)
        self.d.flush()

    def _incr_step(self, key):
        st = self.incr[key]
        chunk = st["data"][st["off"]:st["off"] + INCR_CHUNK]
        st["win"].change_property(key[1], st["target"], 8, chunk)
        st["off"] += len(chunk)
        if not chunk:
            del self.incr[key]
        self.d.flush()

    def stop(self):
        self.stop_ev.set()
        self.join(TIMEOUT)
        self.d.close()


class Requestor:
    """Converts selections on its own display connection."""

    def __init__(self, name):
        self.d = xdisplay.Display(name)
        self.win = make_window(self.d)
        self.prop = self.d.intern_atom("FLAMEWM_CLIPBOARD_PROBE")
        self.incr_atom = self.d.intern_atom("INCR")

    def _wait(self, pred, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            while self.d.pending_events():
                ev = self.d.next_event()
                if pred(ev):
                    return ev
            left = deadline - time.monotonic()
            if left <= 0:
                return None
            select.select([self.d.fileno()], [], [], left)

    def _read_delete(self):
        # Deleting the property is the INCR handshake: the owner sends the next chunk on PropertyDelete.
        reply = self.win.get_full_property(self.prop, X.AnyPropertyType)
        self.win.delete_property(self.prop)
        self.d.flush()
        return reply

    def convert(self, sel, target):
        """Return (value, incr_used); value None if owner refused."""
        sel_a = self.d.intern_atom(sel)
        tgt_a = self.d.intern_atom(target)
        self.win.convert_selection(sel_a, tgt_a, self.prop, X.CurrentTime)
        self.d.flush()
        note = self._wait(lambda e: e.type == X.SelectionNotify and e.selection == sel_a
                          and e.target == tgt_a)
        if note is None:
            raise RuntimeError(f"no SelectionNotify for {sel}/{target}")
        if note.property == X.NONE:
            return None, False
        reply = self._read_delete()
        if reply is not None and reply.property_type == self.incr_atom:
            chunks = []
            while True:
                ev = self._wait(lambda e: e.type == X.PropertyNotify and e.atom == self.prop
                                and e.state == X.PropertyNewValue)
                if ev is None:
                    raise RuntimeError("INCR chunk timeout")
                part = self._read_delete()
                raw = bytes(part.value) if part is not None else b""
                if not raw:
                    break
                chunks.append(raw)
            return b"".join(chunks), True
        if reply is None:
            return b"", False
        if reply.format == 32:
            return [int(v) for v in reply.value], False
        return bytes(reply.value), False

    def close(self):
        self.d.close()


def result(status, detail):
    return {"status": status, "detail": detail}


def run_checks(name):
    payloads = make_payloads()
    checks = {}

    def check(cname, fn):
        try:
            status, detail = fn()
        except Exception as exc:
            status, detail = "FAIL", f"error: {type(exc).__name__}: {exc}"
        checks[cname] = result(status, detail)

    owner = Owner(name)
    owner.start()
    req = Requestor(name)
    try:
        def roundtrip(key):
            def fn():
                exp = payloads[key]
                owner.own("CLIPBOARD", exp)
                got, incr = req.convert("CLIPBOARD", "UTF8_STRING")
                if got is None:
                    return "FAIL", "owner refused UTF8_STRING conversion"
                ok = len(got) == len(exp) and sha(got) == sha(exp)
                return ("PASS" if ok else "FAIL",
                        f"expected sha256={sha(exp)} len={len(exp)} "
                        f"got sha256={sha(got)} len={len(got)} incr={incr}")
            return fn

        def targets():
            owner.own("CLIPBOARD", payloads["ascii"])
            atoms, _ = req.convert("CLIPBOARD", "TARGETS")
            names = [req.d.get_atom_name(a) for a in atoms] if atoms else []
            return ("PASS" if "UTF8_STRING" in names else "FAIL",
                    f"targets={sorted(names)}")

        check("targets_contains_utf8_string", targets)
        for key in ("ascii", "multiline", "unicode", "mib"):
            check(f"clipboard_{key}_roundtrip", roundtrip(key))

        def incr_used():
            owner.own("CLIPBOARD", payloads["mib"])
            _, incr = req.convert("CLIPBOARD", "UTF8_STRING")
            return ("PASS" if incr else "FAIL", f"incr_used={incr}")

        check("clipboard_mib_incr_used", incr_used)

        def primary_independent():
            clip = b"clipboard-only-" + payloads["ascii"]
            prim = b"primary-only-" + payloads["unicode"]
            owner.own("CLIPBOARD", clip)
            mark = len(owner.clears)
            owner.own("PRIMARY", prim)
            got_c, _ = req.convert("CLIPBOARD", "UTF8_STRING")
            got_p, _ = req.convert("PRIMARY", "UTF8_STRING")
            clip_cleared = "CLIPBOARD" in owner.clears[mark:]
            ok = (got_c == clip and got_p == prim and not clip_cleared)
            return ("PASS" if ok else "FAIL",
                    f"clipboard_sha256_ok={got_c == clip} "
                    f"primary_sha256_ok={got_p == prim} clipboard_cleared={clip_cleared}")

        check("primary_independent", primary_independent)

        def selection_clear():
            owner.own("CLIPBOARD", payloads["ascii"])
            mark = len(owner.clears)
            cd = xdisplay.Display(name)
            try:
                cwin = make_window(cd)
                cwin.set_selection_owner(cd.intern_atom("CLIPBOARD"), X.CurrentTime)
                cd.flush()
                deadline = time.monotonic() + TIMEOUT
                while time.monotonic() < deadline and "CLIPBOARD" not in owner.clears[mark:]:
                    time.sleep(0.05)
                owned = cd.get_selection_owner(cd.intern_atom("CLIPBOARD")).id == cwin.id
                delivered = "CLIPBOARD" in owner.clears[mark:]
                ok = owned and delivered
                return ("PASS" if ok else "FAIL",
                        f"takeover_owner_ok={owned} selection_clear_delivered={delivered}")
            finally:
                cd.close()

        check("selection_clear_on_takeover", selection_clear)
    finally:
        req.close()
        owner.stop()
    if owner.error:
        checks["owner_thread"] = result("FAIL", owner.error)
    return checks


def phase_status(checks):
    if any(c["status"] == "FAIL" for c in checks.values()):
        return "FAIL"
    return "PASS"


def start_xvfb(num, env):
    proc = subprocess.Popen(
        ["Xvfb", f":{num}", "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"],
        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, start_new_session=True)
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            return None
        try:
            xdisplay.Display(f":{num}").close()
            return proc
        except xerror.DisplayConnectionError:
            time.sleep(0.1)
    proc.kill()
    return None


def pick_display():
    for n in DISPLAY_RANGE:
        if not display_in_use(str(n)):
            return n
    return None


def stop_proc(proc):
    if proc is None:
        return
    stop_process(proc)


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="X11 CLIPBOARD/PRIMARY acceptance probe on a private Xvfb "
                    "(:90..:99). Prints JSON; exit 0 no FAIL, 1 FAIL, 3 env blocked.")
    ap.add_argument("--with-wm", metavar="WM_BIN", default=None,
                    help="flamewm binary to start on the same display before the with_wm phase; "
                         "if absent or not executable that phase is NOT_RUN")
    args = ap.parse_args(argv)

    report = {"tool": "flamewm_clipboard_acceptance", "status": None, "exit": None,
              "display": None, "phases": {}}

    def finish(code):
        report["exit"] = code
        print(json.dumps(report, indent=2, sort_keys=True))
        return code

    if shutil.which("Xvfb") is None:
        report["status"] = "ENV_BLOCKED"
        report["reason"] = "Xvfb not found on PATH"
        return finish(EXIT_ENV)
    num = pick_display()
    if num is None:
        report["status"] = "ENV_BLOCKED"
        report["reason"] = "no free display in :90..:99"
        return finish(EXIT_ENV)

    tmp = tempfile.mkdtemp(prefix="flamewm-clip-")
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": tmp,
           "XDG_RUNTIME_DIR": tmp, "LANG": "C.UTF-8"}
    display = f":{num}"
    report["display"] = display
    xvfb = None
    wm = None
    try:
        xvfb = start_xvfb(num, env)
        if xvfb is None:
            report["status"] = "ENV_BLOCKED"
            report["reason"] = "Xvfb failed to start"
            return finish(EXIT_ENV)

        try:
            checks = run_checks(display)
            report["phases"]["no_wm"] = {"status": phase_status(checks), "checks": checks}
        except Exception as exc:
            report["phases"]["no_wm"] = {"status": "FAIL",
                                         "error": f"{type(exc).__name__}: {exc}"}

        if args.with_wm is None:
            report["phases"]["with_wm"] = {"status": "NOT_RUN",
                                           "reason": "--with-wm not given"}
        elif not os.access(args.with_wm, os.X_OK):
            report["phases"]["with_wm"] = {"status": "NOT_RUN",
                                           "reason": "WM binary absent or not executable"}
        else:
            try:
                wm = subprocess.Popen([args.with_wm], env=dict(env, DISPLAY=display),
                                      stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL, start_new_session=True)
                time.sleep(2.0)
                if wm.poll() is not None:
                    raise RuntimeError(f"WM exited early with {wm.returncode}")
                checks = run_checks(display)
                report["phases"]["with_wm"] = {"status": phase_status(checks), "checks": checks}
            except Exception as exc:
                report["phases"]["with_wm"] = {"status": "FAIL",
                                               "error": f"{type(exc).__name__}: {exc}"}

        statuses = [p["status"] for p in report["phases"].values()]
        report["status"] = "FAIL" if "FAIL" in statuses else "PASS"
        return finish(EXIT_FAIL if "FAIL" in statuses else EXIT_OK)
    finally:
        stop_proc(wm)
        stop_proc(xvfb)
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
