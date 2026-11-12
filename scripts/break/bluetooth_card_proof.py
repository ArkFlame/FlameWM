#!/usr/bin/env python3
"""Executed proof for the Bluetooth pairing card on a private Xvfb and a private dbus-daemon.

A mock org.bluez registers the shell's agent, then calls Agent1 methods one step at a time. The
driver waits for each step, measures the card, presses keys with the XTest helper (key.py), and
checks the reply the agent sent back on the bus. Nothing here touches the host display or bus.

Usage: bluetooth_card_proof.py SHELL_BINARY OUT_DIR
Writes OUT_DIR/events.jsonl, OUT_DIR/shell.log, and prints one PASS/FAIL line per check.
"""

import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TITLE = "FlameWM Bluetooth pairing"
DEVICE = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
PASSKEY = 123456


def mock(address, out_dir):
    """Mock org.bluez on the private bus. Runs one agent step per `next` line in control.txt."""
    import dbus
    import dbus.mainloop.glib
    import dbus.service
    from gi.repository import GLib

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(address)
    events = os.path.join(out_dir, "events.jsonl")
    control = os.path.join(out_dir, "control.txt")
    state = {"agent": None, "started": 0, "busy": False}

    def log(record):
        record["t"] = time.time()
        with open(events, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(record) + "\n")

    steps = [
        ("RequestPasskey", lambda: [dbus.ObjectPath(DEVICE)]),
        ("RequestConfirmation", lambda: [dbus.ObjectPath(DEVICE), dbus.UInt32(PASSKEY)]),
        ("DisplayPasskey", lambda: [dbus.ObjectPath(DEVICE), dbus.UInt32(PASSKEY), dbus.UInt16(0)]),
        ("RequestAuthorization", lambda: [dbus.ObjectPath(DEVICE)]),
        ("RequestPinCode", lambda: [dbus.ObjectPath(DEVICE)]),
    ]

    def run_step():
        if state["busy"] or state["agent"] is None:
            return True
        with open(control, encoding="utf-8") as handle:
            lines = [line for line in handle.read().splitlines() if line.strip()]
        if len(lines) <= state["started"] or state["started"] >= len(steps):
            return True
        index = state["started"]
        name, args = steps[index]
        state["started"] += 1
        state["busy"] = True
        log({"ev": "pending", "step": index, "name": name})
        agent = dbus.Interface(bus.get_object(state["agent"], "/org/flamewm/BluetoothAgent"), "org.bluez.Agent1")

        def ok(*values):
            state["busy"] = False
            log({"ev": "done", "step": index, "name": name, "kind": "reply", "value": [repr(v) for v in values]})

        def err(error):
            state["busy"] = False
            log({"ev": "done", "step": index, "name": name, "kind": "error", "value": str(error)})

        getattr(agent, name)(*args(), reply_handler=ok, error_handler=err, timeout=120)
        return True

    class Manager(dbus.service.Object):
        @dbus.service.method("org.bluez.AgentManager1", in_signature="os", out_signature="", sender_keyword="sender")
        def RegisterAgent(self, path, capability, sender=None):
            log({"ev": "register", "path": str(path), "capability": str(capability)})

        @dbus.service.method("org.bluez.AgentManager1", in_signature="o", out_signature="", sender_keyword="sender")
        def RequestDefaultAgent(self, path, sender=None):
            state["agent"] = sender
            log({"ev": "default", "path": str(path), "sender": sender})

        @dbus.service.method("org.bluez.AgentManager1", in_signature="o", out_signature="")
        def UnregisterAgent(self, path):
            log({"ev": "unregister", "path": str(path)})

    class Device(dbus.service.Object):
        """Device1 properties: the agent resolves the card name from Alias."""

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
        def GetAll(self, interface):
            log({"ev": "props", "path": DEVICE, "interface": str(interface)})
            return {"Alias": dbus.String("Test Keyboard"), "Name": dbus.String("Keyboard")}

    bus.request_name("org.bluez")
    Manager(bus, "/org/bluez")
    Device(bus, DEVICE)
    GLib.timeout_add(200, run_step)
    loop = GLib.MainLoop()
    loop.run()


def wait_for(predicate, timeout, what):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    raise SystemExit(f"FAIL timeout waiting for {what}")


def find_card(display):
    from Xlib import X, display as xdisplay

    dpy = xdisplay.Display(display)
    found = []

    def walk(window):
        try:
            name = window.get_wm_name()
            if name == TITLE:
                attrs = window.get_attributes()
                if attrs.map_state == X.IsViewable:
                    geometry = window.get_geometry()
                    found.append((geometry.width, geometry.height))
        except Exception:
            pass
        try:
            for child in window.query_tree().children:
                walk(child)
        except Exception:
            pass

    walk(dpy.screen().root)
    dpy.close()
    return found[0] if found else None


def keys(display, *chords):
    env = dict(os.environ, DISPLAY=display)
    subprocess.run([sys.executable, os.path.join(HERE, "key.py"), *chords], env=env, check=True)


def read_events(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def run(shell, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    home = os.path.join(out_dir, "home")
    os.makedirs(home, exist_ok=True)
    display_num = next(n for n in range(150, 200) if not os.path.exists(f"/tmp/.X11-unix/X{n}"))
    display = f":{display_num}"
    socket = os.path.join(out_dir, "bus.sock")
    procs = []
    try:
        xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-noreset"],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        procs.append(xvfb)
        wait_for(lambda: os.path.exists(f"/tmp/.X11-unix/X{display_num}"), 15, "Xvfb")
        address = f"unix:path={socket}"
        daemon = subprocess.run(["dbus-daemon", "--config-file=/usr/share/dbus-1/session.conf",
                                 f"--address={address}", "--nopidfile", "--print-pid=1", "--fork"],
                                capture_output=True, text=True, check=True)
        dbus_pid = int(daemon.stdout.split()[-1])
        env = dict(os.environ, DISPLAY=display, HOME=home, XDG_RUNTIME_DIR=out_dir,
                   DBUS_SYSTEM_BUS_ADDRESS=address, DBUS_SESSION_BUS_ADDRESS=address,
                   XDG_SESSION_ID="1")
        for name in ("events.jsonl", "control.txt"):
            path = os.path.join(out_dir, name)
            if os.path.exists(path):
                os.remove(path)
        open(os.path.join(out_dir, "control.txt"), "w", encoding="utf-8").close()
        mock_proc = subprocess.Popen([sys.executable, __file__, "--mock", address, out_dir],
                                     env=env, stdout=open(os.path.join(out_dir, "mock.log"), "w"),
                                     stderr=subprocess.STDOUT)
        procs.append(mock_proc)
        time.sleep(2)  # the mock must own org.bluez before the shell registers its agent
        shell_log = open(os.path.join(out_dir, "shell.log"), "w")
        shell_proc = subprocess.Popen([shell], env=env, stdout=shell_log, stderr=subprocess.STDOUT)
        procs.append(shell_proc)
        events = os.path.join(out_dir, "events.jsonl")
        wait_for(lambda: any(e["ev"] == "default" for e in read_events(events)), 60, "RequestDefaultAgent")
        verdicts = []

        def check(label, ok, detail):
            verdicts.append(ok)
            print(("PASS" if ok else "FAIL"), label, detail, flush=True)

        def next_step(index):
            with open(os.path.join(out_dir, "control.txt"), "a", encoding="utf-8") as handle:
                handle.write(f"next {index}\n")

        def done(index):
            return next((e for e in read_events(events) if e["ev"] == "done" and e["step"] == index), None)

        check("registered", any(e["ev"] == "register" for e in read_events(events)), "RegisterAgent seen")

        # Step 0: RequestPasskey. Typed digits must reach the agent as the reply value.
        next_step(0)
        wait_for(lambda: any(e["ev"] == "pending" and e["step"] == 0 for e in read_events(events)), 30, "step 0")
        card = wait_for(lambda: find_card(display), 30, "pairing card (passkey)")
        check("card size 420 wide", card[0] == 420, f"card={card[0]}x{card[1]}")
        keys(display, *list("123456"), "Return")
        reply = wait_for(lambda: done(0), 30, "step 0 reply")
        check("typed passkey reached agent", reply["kind"] == "reply" and "UInt32" in str(reply["value"]) and str(PASSKEY) in str(reply["value"]), str(reply["value"]))
        wait_for(lambda: not find_card(display), 15, "card closed after accept")

        # Step 1: RequestConfirmation. Escape must reject with org.bluez.Error.Rejected.
        next_step(1)
        wait_for(lambda: any(e["ev"] == "pending" and e["step"] == 1 for e in read_events(events)), 30, "step 1")
        card = wait_for(lambda: find_card(display), 30, "pairing card (confirm)")
        check("confirm card size 420 wide", card[0] == 420, f"card={card[0]}x{card[1]}")
        keys(display, "Escape")
        reply = wait_for(lambda: done(1), 30, "step 1 reply")
        check("Escape rejects confirmation", reply["kind"] == "error" and "org.bluez.Error.Rejected" in reply["value"], reply["value"])
        wait_for(lambda: not find_card(display), 15, "card closed after reject")

        # Step 2: DisplayPasskey is a notice, acknowledged at once. The card shows it until Escape.
        next_step(2)
        wait_for(lambda: any(e["ev"] == "pending" and e["step"] == 2 for e in read_events(events)), 30, "step 2")
        card = wait_for(lambda: find_card(display), 30, "notice card")
        check("notice card size 420 wide", card[0] == 420, f"card={card[0]}x{card[1]}")
        reply = wait_for(lambda: done(2), 30, "step 2 ack")
        check("display notice acknowledged at once", reply["kind"] == "reply", str(reply["value"]))
        keys(display, "Escape")
        wait_for(lambda: not find_card(display), 15, "notice dismissed")
        check("Escape dismisses notice", True, "card closed")

        # Step 3: RequestAuthorization, accepted with Return and answered with an empty reply.
        next_step(3)
        wait_for(lambda: any(e["ev"] == "pending" and e["step"] == 3 for e in read_events(events)), 30, "step 3")
        card = wait_for(lambda: find_card(display), 30, "authorize card")
        check("authorize card size 420 wide", card[0] == 420, f"card={card[0]}x{card[1]}")
        keys(display, "Return")
        reply = wait_for(lambda: done(3), 30, "step 3 reply")
        check("Return accepts authorization", reply["kind"] == "reply", str(reply["value"]))
        check("agent asked Device1 for its name", any(e["ev"] == "props" for e in read_events(events)), "GetAll on device")

        # Step 4: RequestPinCode with no key press. The card waits, then the agent rejects at the
        # production 30 s prompt timeout (no injected timeout here) and the card closes.
        next_step(4)
        pending = wait_for(lambda: next((e for e in read_events(events) if e["ev"] == "pending" and e["step"] == 4), None), 30, "step 4")
        card = wait_for(lambda: find_card(display), 30, "pairing card (PIN, unanswered)")
        check("PIN card size 420 wide", card[0] == 420, f"card={card[0]}x{card[1]}")
        reply = wait_for(lambda: done(4), 90, "step 4 expiry")
        elapsed = reply["t"] - pending["t"]
        check("unanswered prompt expires to Rejected", reply["kind"] == "error" and "org.bluez.Error.Rejected" in reply["value"], reply["value"])
        check("expiry after the 30 s prompt timeout", 29.0 <= elapsed <= 45.0, f"elapsed={elapsed:.1f}s")
        wait_for(lambda: not find_card(display), 15, "card closed after expiry")
        check("card closed after expiry", True, "no card on the X display")
        return all(verdicts)
    finally:
        for proc in reversed(procs):
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        if "dbus_pid" in locals():
            try:
                os.kill(dbus_pid, 15)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--mock":
        mock(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 3:
        sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
